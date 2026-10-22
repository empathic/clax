// The C functions called as a C caller calls them, with NULL where the
// header says a pointer is never NULL: each refuses at once (open answers
// 0, call answers 2), and nothing is posted.

#include <stdio.h>

#include "clax_embed.h"

static int posted = 0;
static int failures = 0;

static int8_t post(int64_t port, Dart_CObject *message) {
    (void)port;
    (void)message;
    posted++;
    return 1;
}

static void expect(int ok, const char *what) {
    if (!ok) {
        fprintf(stderr, "FAIL: %s\n", what);
        failures++;
    }
}

int main(void) {
    static const uint8_t home[] = "/nonexistent/clax-embed-nulls-test";
    static const uint8_t frame[] = "CLX1";

    expect(clax_embed_open(NULL, 0, post, 1) == 0, "open with a NULL home and no length answers 0");
    expect(clax_embed_open(NULL, 8, post, 1) == 0, "open with a NULL home answers 0");
    expect(clax_embed_open(home, sizeof home - 1, NULL, 1) == 0, "open with a NULL post answers 0");
    expect(clax_embed_call(1, NULL, 0, 2, 3) == 2, "call with a NULL frame and no length answers 2");
    expect(clax_embed_call(1, NULL, 64, 2, 3) == 2, "call with a NULL frame answers 2");
    expect(clax_embed_call(0, frame, 0, 2, 3) == 1, "call with an empty frame on no core answers 1");
    expect(posted == 0, "nothing is posted");
    if (failures == 0) {
        printf("clax_embed: every NULL is refused\n");
    }
    return failures == 0 ? 0 : 1;
}
