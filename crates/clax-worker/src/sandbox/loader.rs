//! The JavaScript side of a run, wasm32 only: the dynamic Worker loaded,
//! its `run` called with the instance's `host` function, and the loop that
//! answers its messages (see the module docs of [`super`]).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Poll, Waker};
use std::time::Duration;

use clax_bot_sandbox::{CancelToken, Host, Limits, Script, ScriptOutcome};
use futures_util::future::{Either, select};
use js_sys::{Array, Function, Object, Promise, Reflect};
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::{
    COMPATIBILITY_DATE, DRIVER_MODULE, End, PRELUDE_MODULE, RunState, STOP_REPLY, Step,
    driver_module, job_json, module_name, ms_to_us, prelude_module, script_module,
};

/// What the `host` function shares with the run's loop.
#[derive(Default)]
struct Inbox {
    /// Messages from the isolate, each with its answer's `resolve`.
    queue: VecDeque<(String, Function)>,
    waker: Option<Waker>,
    /// Once the run has ended: every message is answered with a stop.
    closed: bool,
}

fn message(e: &JsValue) -> String {
    e.dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| e.as_string())
        .unwrap_or_else(|| format!("{e:?}"))
}

fn set(o: &Object, key: &str, value: &JsValue) -> Result<(), String> {
    Reflect::set(o, &JsValue::from_str(key), value)
        .map(|_| ())
        .map_err(|e| message(&e))
}

/// Calls `o[method](...args)` through `Reflect.apply`: `o` may be an RPC
/// stub, on which every property of a method is another remote call.
fn call(o: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, String> {
    let f: Function = Reflect::get(o, &JsValue::from_str(method))
        .map_err(|e| message(&e))?
        .dyn_into()
        .map_err(|_| format!("the loader has no {method}"))?;
    let argv: Array = args.iter().collect();
    Reflect::apply(&f, o, &argv).map_err(|e| message(&e))
}

/// The `WorkerCode` of one script.
fn worker_code(script: &Script, limits: &Limits, name: &str) -> Result<Object, String> {
    let code = Object::new();
    set(&code, "compatibilityDate", &COMPATIBILITY_DATE.into())?;
    set(&code, "mainModule", &DRIVER_MODULE.into())?;
    let modules = Object::new();
    set(&modules, DRIVER_MODULE, &driver_module(name).into())?;
    let cjs = |source: String| -> Result<JsValue, String> {
        let m = Object::new();
        set(&m, "cjs", &source.into())?;
        Ok(m.into())
    };
    set(&modules, PRELUDE_MODULE, &cjs(prelude_module())?)?;
    set(&modules, name, &cjs(script_module(&script.code))?)?;
    set(&code, "modules", &modules)?;
    set(&code, "env", &Object::new())?;
    set(&code, "globalOutbound", &JsValue::NULL)?;
    let cpu = Object::new();
    set(&cpu, "cpuMs", &JsValue::from_f64(limits.cpu_ms as f64))?;
    set(&code, "limits", &cpu)?;
    Ok(code)
}

/// How long a run whose isolate failed waits for the driver's last
/// message, in milliseconds.
const FINAL_GRACE_MS: f64 = 50.0;

enum Event {
    Cancel,
    Watchdog,
    Message(String, Function),
    Returned(Result<JsValue, JsValue>),
}

/// Runs `script` in a fresh dynamic Worker from `binding` (see the module
/// docs of [`super`]).
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    binding: &JsValue,
    script: &Script,
    ops: &[String],
    limits: &Limits,
    mut state: RunState,
    host: &dyn Host,
    cancel: &CancelToken,
    deadline_ms: u64,
    started: f64,
) -> ScriptOutcome {
    let wall = || ms_to_us(js_sys::Date::now() - started);
    let name = module_name(&script.title);
    let inbox = Rc::new(RefCell::new(Inbox::default()));
    let host_fn = {
        let inbox = inbox.clone();
        Closure::<dyn FnMut(JsValue) -> Promise>::new(move |text: JsValue| {
            let text = text.as_string().unwrap_or_default();
            let inbox = inbox.clone();
            Promise::new(&mut |resolve: Function, _reject: Function| {
                let mut inbox = inbox.borrow_mut();
                if inbox.closed {
                    let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(STOP_REPLY));
                    return;
                }
                inbox.queue.push_back((text.clone(), resolve));
                if let Some(waker) = inbox.waker.take() {
                    waker.wake();
                }
            })
        })
        // Freed by the JavaScript GC once the isolate lets go of it: an
        // abandoned isolate may still call it, and is answered a stop.
        .into_js_value()
    };
    let returned = worker_code(script, limits, &name).and_then(|code| {
        let stub = call(binding, "load", &[code.into()])?;
        let entrypoint = call(&stub, "getEntrypoint", &[JsValue::from_str("Run")])?;
        let job = job_json(ops, limits, &name, script.determinism);
        call(
            &entrypoint,
            "run",
            &[JsValue::from_str(&job), host_fn.clone()],
        )
    });
    let returned = match returned {
        Ok(r) => r,
        Err(e) => {
            return state.outcome(
                End::NotLoaded(format!("Failed to start Worker: {e}")),
                0,
                wall(),
            );
        }
    };
    let mut returned = JsFuture::from(Promise::resolve(&returned));
    let mut busy_ms = 0.0f64;
    // What the isolate's request failed with, while the run waits up to
    // FINAL_GRACE_MS for the last message the driver sent just before it
    // aborted its isolate, which may arrive after the failure.
    let mut failed: Option<String> = None;
    let end = loop {
        let left = match failed {
            Some(_) => FINAL_GRACE_MS,
            None => (deadline_ms as f64 - busy_ms).max(0.0),
        };
        let mut timer = Box::pin(worker::Delay::from(Duration::from_millis(left as u64)));
        let mut cancelled = Box::pin(cancel.cancelled());
        let waited = js_sys::Date::now();
        let event = std::future::poll_fn(|cx| {
            if cancelled.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Event::Cancel);
            }
            {
                let mut inbox = inbox.borrow_mut();
                if let Some((text, resolve)) = inbox.queue.pop_front() {
                    return Poll::Ready(Event::Message(text, resolve));
                }
                inbox.waker = Some(cx.waker().clone());
            }
            if failed.is_none()
                && let Poll::Ready(r) = Pin::new(&mut returned).poll(cx)
            {
                return Poll::Ready(Event::Returned(r));
            }
            if timer.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Event::Watchdog);
            }
            Poll::Pending
        })
        .await;
        if failed.is_none() {
            busy_ms += (js_sys::Date::now() - waited).max(0.0);
        }
        match event {
            Event::Cancel => break End::Cancelled,
            Event::Watchdog => {
                if let Some(message) = failed.take() {
                    break End::Failed(message);
                }
                break End::Watchdog {
                    busy_ms: busy_ms as u64,
                };
            }
            Event::Returned(Ok(v)) => match state.take(&v.as_string().unwrap_or_default()) {
                Step::End(end) => break end,
                _ => break End::Garbled("the run answered a call".into()),
            },
            Event::Returned(Err(e)) => failed = Some(message(&e)),
            Event::Message(text, resolve) => match state.take(&text) {
                Step::End(end) => {
                    let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(STOP_REPLY));
                    break end;
                }
                Step::Reply(answer) => {
                    let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(&answer));
                }
                // Past the watchdog's time a call waiting beside its timer
                // is not made.
                Step::Call { .. } if busy_ms >= deadline_ms as f64 => {
                    let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(STOP_REPLY));
                    break End::Watchdog {
                        busy_ms: busy_ms as u64,
                    };
                }
                Step::Call {
                    op,
                    args,
                    args_bytes,
                } => {
                    let t = js_sys::Date::now();
                    let result =
                        match select(Box::pin(host.call(&op, args)), Box::pin(cancel.cancelled()))
                            .await
                        {
                            Either::Left((r, _)) => Some(r),
                            Either::Right(((), _)) => None,
                        };
                    let us = ms_to_us(js_sys::Date::now() - t);
                    let Some(result) = result else {
                        state.interrupted(op, args_bytes, us);
                        let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(STOP_REPLY));
                        break End::Cancelled;
                    };
                    let answer = state.answer(op, args_bytes, us, &result);
                    let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(&answer));
                }
            },
        }
    };
    // Whatever the isolate sends from now on is told the run has ended.
    {
        let mut inbox = inbox.borrow_mut();
        inbox.closed = true;
        for (_, resolve) in inbox.queue.drain(..) {
            let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(STOP_REPLY));
        }
    }
    state.outcome(end, ms_to_us(busy_ms), wall())
}
