/* Allocation shim for libcaption.
 *
 * libcaption's structs are declared in its headers and are neither small nor
 * stable enough to be worth mirroring in Rust. Rather than hard coding their
 * sizes, which would corrupt memory the moment upstream changed a field, the
 * allocation happens here where the real headers are in scope.
 *
 * Rust therefore only ever holds opaque pointers.
 */

#include "caption.h"
#include "mpeg.h"
#include <stdlib.h>

caption_frame_t* fakestream_caption_frame_new(void)
{
    caption_frame_t* frame = calloc(1, sizeof(caption_frame_t));
    if (frame) {
        caption_frame_init(frame);
    }
    return frame;
}

void fakestream_caption_frame_free(caption_frame_t* frame)
{
    free(frame);
}

sei_t* fakestream_sei_new(double timestamp)
{
    sei_t* sei = calloc(1, sizeof(sei_t));
    if (sei) {
        sei_init(sei, timestamp);
    }
    return sei;
}

/* Releases the message list libcaption allocated, then the struct itself. */
void fakestream_sei_free(sei_t* sei)
{
    if (sei) {
        sei_free(sei);
        free(sei);
    }
}

/* sei_message_head is a static inline in the header, so it has no linkable
 * symbol. The other accessors are ordinary functions and need no wrapper. */
sei_message_t* fakestream_sei_message_head(sei_t* sei)
{
    return sei_message_head(sei);
}
