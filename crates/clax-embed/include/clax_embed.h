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
// handle at once (never 0), or 0 when that home is open in this process
// already, the path is not UTF-8, or `home` or `post` is NULL. The store
// opens, backs up and migrates on the core's own threads; `port` hears
// {"type":"ready"} or {"type":"failed","code":…,"message":…}, then
// {"type":"closed"} after clax_embed_close. `post` is Dart's
// NativeApi.postCObject.
uint64_t clax_embed_open(const uint8_t *home, size_t home_len,
                         ClaxPost post, int64_t port);

// Sends one request frame (§2.3). Returns 0 when the core took it, 1 for
// an unknown handle, 2 for a frame that does not parse or a NULL `req`.
// The response frame is posted to `port` as one Uint8List: `tag` (8
// bytes, little endian) then the frame. A call made before "ready" waits
// for it.
int32_t clax_embed_call(uint64_t core, const uint8_t *req, size_t req_len,
                        int64_t port, int64_t tag);

// Posts the core's events to `port`, one Uint8List per SSE block of the
// stream hub (§2.4), until the port is closed or the subscription ends;
// an ending subscription posts `event: end` with {"reason":…} last
// (core_failed, core_closed, stream_refused, stream_ended). 0, or 1 for
// an unknown handle.
int32_t clax_embed_subscribe(uint64_t core, int64_t port);

// Stops the core as `clax serve` stops on SIGTERM: in-flight calls end,
// tasks stop, the journal syncs, the store closes. Returns at once; the
// open port hears {"type":"closed"}. 0, or 1 for an unknown handle.
int32_t clax_embed_close(uint64_t core);

#endif
