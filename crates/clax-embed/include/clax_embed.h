// The embedded Clax core (spec 2026-10-19 §2.2).
#ifndef CLAX_EMBED_H
#define CLAX_EMBED_H

#include <stdint.h>
#include <stddef.h>
typedef struct Dart_CObject Dart_CObject;
typedef int8_t (*ClaxPost)(int64_t port, Dart_CObject *message);

// Pointers are never NULL: a NULL `home` or `post` makes clax_embed_open
// answer 0, and a NULL `req` makes clax_embed_call answer 2, before either
// reads anything. A length may be 0 with a pointer that is not NULL.

// Opens the core on the home at home[0..home_len] (UTF-8) and returns its
// handle at once (never 0). The store opens, backs up and migrates on the
// core's own threads; `port` hears {"type":"ready"} or
// {"type":"failed","code":…,"message":…}, then {"type":"closed"} after
// clax_embed_close. A home this process has open already (the app's own
// core) is not opened again. When the isolate that opened it is gone (it
// restarted without closing the core) the open re-attaches, returning
// that core's handle; its open events go to `port` from then on, which
// hears the latest at once ("ready", or where opening has got to). To
// tell, the open posts {"type":"probe"}, which is no event and which a
// client ignores, to the core's open port: the post fails only once that
// port's isolate is gone. A core whose open failed is never re-attached
// to. Opens are taken one at a time, so two at once of one home get one
// handle and one refusal. 0 when the path is not UTF-8 (`port` hears
// "failed" with code open_failed), that home's core has a live client
// (code home_in_use, the home open in this app) or is closing (code
// home_in_use), or `home` or `post` is NULL (nothing is posted). `post`
// is Dart's NativeApi.postCObject.
uint64_t clax_embed_open(const uint8_t *home, size_t home_len,
                         ClaxPost post, int64_t port);

// Sends one request frame (§2.3). Returns 0 when the core took it, 1 for
// an unknown handle, 2 for a frame that does not parse or a NULL `req`.
// The response frame is posted to `port` as one Uint8List: `tag` (8
// bytes, little endian) then the frame. A call made before "ready" waits
// for it. A cancel frame, {"cancel": <tag>}, is answered 204 at once
// under its own `tag`, after it aborts the call sent to this `port` with
// that tag, if the core is still running it: that call is never
// answered, and a write it had not committed does not happen.
int32_t clax_embed_call(uint64_t core, const uint8_t *req, size_t req_len,
                        int64_t port, int64_t tag);

// Posts the core's events to `port`, one Uint8List per SSE block of the
// stream hub (§2.4), until the port is closed or the subscription ends;
// an ending subscription posts `event: end` with {"reason":…} last
// (core_failed, core_closed, stream_refused, stream_ended, internal). 0,
// or 1 for an unknown handle.
int32_t clax_embed_subscribe(uint64_t core, int64_t port);

// Stops the core as `clax serve` stops on SIGTERM: new calls answer 503
// core_closed, streams and long polls are told to end, calls in flight
// finish for up to 5 s (then answer 503 core_closed), tasks stop, the
// journal syncs, the store closes. Returns at once; the open port hears
// {"type":"closed"}, after the answer of every call that finished in the
// drain has been posted. 0, or 1 for an unknown handle.
int32_t clax_embed_close(uint64_t core);

#endif
