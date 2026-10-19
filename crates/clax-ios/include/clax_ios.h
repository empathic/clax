// The Clax daemon inside an iOS app's process (crates/clax-ios).
#ifndef CLAX_IOS_H
#define CLAX_IOS_H

#include <stdint.h>

// Starts the daemon on its own thread, asking for `port` on 127.0.0.1
// (0: any free port), with the home under $HOME. Returns at once: 0
// started, 1 already running (starting, up or stopping), 2 no $HOME.
int32_t clax_start(uint16_t port);
// Asks a running daemon to stop; returns at once. clax_state() says when
// it has.
void clax_stop(void);
// 0 never started, 1 starting, 2 up, 3 stopping, or 100 + the exit status
// `clax serve` would have exited with (100 a clean stop, 165 a failed
// migration, 101 any other error, 201 a panic on the daemon's thread).
int32_t clax_state(void);
// Under memory pressure and on entering the background: SQLite's caches
// shrink and the WAL is checkpointed (PASSIVE). Safe from any thread;
// returns when done, after 2 s, or at once when no daemon is up.
void clax_trim_memory(void);

#endif
