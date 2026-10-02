//! Limits on `sample()` spending (spec §14, §18; `sample.d.ts`): a count of
//! calls that reached the provider per artifact per local day, stopped at
//! `daily_call_cap`, and a per-viewer queue in which two calls run, four more
//! wait their turn, and the rest are refused `rate_limited`. Counts live in
//! memory and restart with the daemon.

use chrono::NaiveDate;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const RUNNING_PER_VIEWER: usize = 2;
pub const WAITING_PER_VIEWER: usize = 4;

#[derive(Default)]
pub struct CallCounts {
    days: Mutex<HashMap<String, (NaiveDate, u32)>>,
}

impl CallCounts {
    /// Calls counted for `aid` on `date`.
    pub fn today(&self, aid: &str, date: NaiveDate) -> u32 {
        match self.days.lock().expect("counts lock").get(aid) {
            Some((d, n)) if *d == date => *n,
            _ => 0,
        }
    }

    /// Counts one more call for `aid` on `date` and returns the new count, or
    /// `Err(cap)` without counting when the count has reached `cap`.
    pub fn try_take(&self, aid: &str, date: NaiveDate, cap: Option<u32>) -> Result<u32, u32> {
        let mut days = self.days.lock().expect("counts lock");
        let entry = days.entry(aid.to_string()).or_insert((date, 0));
        if entry.0 != date {
            *entry = (date, 0);
        }
        if let Some(c) = cap
            && entry.1 >= c
        {
            return Err(c);
        }
        entry.1 += 1;
        Ok(entry.1)
    }
}

struct Queue {
    running: Arc<Semaphore>,
    waiting: AtomicUsize,
}

#[derive(Default)]
pub struct ViewerQueues {
    queues: Mutex<HashMap<String, Arc<Queue>>>,
}

struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ViewerQueues {
    /// A running slot for `viewer`, waiting for one if two are taken; `None`
    /// at once when four calls already wait. Dropping the future gives up the place.
    pub async fn enter(&self, viewer: &str) -> Option<OwnedSemaphorePermit> {
        let q = self
            .queues
            .lock()
            .expect("queues lock")
            .entry(viewer.to_string())
            .or_insert_with(|| {
                Arc::new(Queue {
                    running: Arc::new(Semaphore::new(RUNNING_PER_VIEWER)),
                    waiting: AtomicUsize::new(0),
                })
            })
            .clone();
        if let Ok(p) = q.running.clone().try_acquire_owned() {
            return Some(p);
        }
        if q.waiting.fetch_add(1, Ordering::SeqCst) >= WAITING_PER_VIEWER {
            q.waiting.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        let _place = Waiting(&q.waiting);
        q.running.clone().acquire_owned().await.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_per_artifact_per_day_and_stop_at_the_cap() {
        let c = CallCounts::default();
        let d1 = NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        let d2 = d1.succ_opt().unwrap();
        assert_eq!(c.try_take("a", d1, Some(2)), Ok(1));
        assert_eq!(c.try_take("a", d1, Some(2)), Ok(2));
        assert_eq!(c.try_take("a", d1, Some(2)), Err(2));
        assert_eq!(c.today("a", d1), 2);
        assert_eq!(c.try_take("b", d1, Some(2)), Ok(1));
        assert_eq!(c.today("a", d2), 0);
        assert_eq!(c.try_take("a", d2, Some(2)), Ok(1));
        assert_eq!(c.try_take("a", d2, None), Ok(2));
        assert_eq!(c.try_take("z", d1, Some(0)), Err(0));
    }

    #[tokio::test]
    async fn two_run_four_wait_and_the_rest_are_refused() {
        let q = Arc::new(ViewerQueues::default());
        let a = q.enter("v").await.unwrap();
        let _b = q.enter("v").await.unwrap();
        let mut waiters = Vec::new();
        for _ in 0..WAITING_PER_VIEWER {
            let q = q.clone();
            waiters.push(tokio::spawn(async move { q.enter("v").await.is_some() }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(q.enter("v").await.is_none());
        assert!(q.enter("w").await.is_some());
        drop(a);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(waiters.iter().filter(|w| w.is_finished()).count() >= 1);
        for w in &waiters {
            w.abort();
        }
    }

    #[tokio::test]
    async fn a_waiter_that_gives_up_frees_its_place() {
        let q = Arc::new(ViewerQueues::default());
        let _a = q.enter("v").await.unwrap();
        let _b = q.enter("v").await.unwrap();
        let mut waiters: Vec<_> = (0..WAITING_PER_VIEWER)
            .map(|_| {
                let q = q.clone();
                tokio::spawn(async move { q.enter("v").await.is_some() })
            })
            .collect();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        waiters.pop().unwrap().abort();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let q2 = q.clone();
        let again = tokio::spawn(async move { q2.enter("v").await.is_some() });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            !again.is_finished(),
            "the freed place is taken by a waiter, not refused"
        );
        again.abort();
        for w in &waiters {
            w.abort();
        }
    }
}
