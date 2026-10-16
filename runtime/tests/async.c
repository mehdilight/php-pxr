/* Hand-written resumable frames exercise the compiler's future runtime ABI. */
#include <sys/socket.h>
const char pxr_php_worker_source[] = "";
pxr_str *pxr_native_object_to_string(void *object) { (void)object; abort(); }

static int cleaned, ran, events[20000], event_count;
static void event(int value) { assert(event_count < 20000); events[event_count++] = value; }
typedef struct { uint64_t now; int waits, interrupts, fail; } fake_clock;
static uint64_t fake_now(void *context) { return ((fake_clock *)context)->now; }
static int fake_wait(struct pollfd *fds, nfds_t count, int timeout, void *context) {
    (void)fds; assert(!count); assert(timeout >= 0);
    fake_clock *clock = context;
    clock->waits++;
    if (clock->interrupts) {
        clock->interrupts--; clock->now += 1000000;
        errno = EINTR; return -1;
    }
    if (clock->fail) { errno = EIO; return -1; }
    clock->now += (uint64_t)timeout * 1000000;
    return 0;
}
static void fake_init(pxr_loop *loop, fake_clock *clock) {
    pxr_loop_init(loop); loop->now = fake_now; loop->wait = fake_wait; loop->clock_context = clock;
}
static void clean(pxr_task *task, void *raw) {
    (void)task; (void)raw; cleaned++;
    pxr_gc_collect(); /* cleanup itself may allocate/call native code */
}
static void end_loop(pxr_loop *loop) {
    pxr_loop_destroy(loop);
    pxr_gc_collect();
    assert(!pxr_heap_blocks && !pxr_heap_bytes && !pxr_async_loops);
}

typedef struct { int pc, value; uint64_t delay; pxr_str *text; } timer_frame;
static void timer(pxr_task *task, void *raw) {
    timer_frame *frame = raw;
    ran++;
    if (!frame->pc++) {
        event(frame->value);
        frame->text = pxr_str_alloc(4); memcpy(frame->text->data, "live", 4);
        task->roots[0] = frame->text; pxr_task_roots(task, 1);
        pxr_task_sleep(task, frame->delay); return;
    }
    assert(frame->text->len == 4 && !memcmp(frame->text->data, "live", 4));
    event(frame->value + 10);
    pxr_task_finish(task, PXR_TAG_STR, (intptr_t)frame->text);
}
static pxr_task *new_timer(pxr_task *parent, int value, uint64_t delay) {
    pxr_task *task = pxr_task_new(parent->loop, parent, sizeof(timer_frame), 1, timer, clean);
    timer_frame *frame = task->frame; frame->value = value; frame->delay = delay;
    return task;
}
typedef struct { int pc; pxr_task *a, *b; } pair_frame;
static void overlap(pxr_task *task, void *raw) {
    pair_frame *frame = raw;
    switch (frame->pc++) {
        case 0:
            frame->a = new_timer(task, 1, 100);
            frame->b = new_timer(task, 2, 20);
            pxr_task_start(frame->a); pxr_task_start(frame->b);
            pxr_task_await(task, frame->a); return;
        case 1: {
            pxr_task_result result = pxr_task_take(frame->a);
            pxr_str *text = (void *)(intptr_t)result.bits;
            assert(result.status == PXR_TASK_OK && text->len == 4);
            assert(event_count == 4 && events[2] == 12 && events[3] == 11);
            pxr_task_await(task, frame->b); return;
        }
        default:
            assert(pxr_task_take(frame->b).status == PXR_TASK_OK);
            pxr_task_finish(task, PXR_TAG_INT, 42);
    }
}
static void sequential(pxr_task *task, void *raw) {
    pair_frame *frame = raw;
    switch (frame->pc++) {
        case 0: frame->a = new_timer(task, 1, 100); pxr_task_await(task, frame->a); return;
        case 1:
            assert(pxr_task_take(frame->a).status == PXR_TASK_OK);
            frame->b = new_timer(task, 2, 20); pxr_task_await(task, frame->b); return;
        default:
            assert(pxr_task_take(frame->b).status == PXR_TASK_OK);
            pxr_task_finish(task, PXR_TAG_INT, 42);
    }
}
static void test_timers(void) {
    for (int sequential_mode = 0; sequential_mode < 2; sequential_mode++) {
        pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock);
        cleaned = ran = event_count = 0;
        pxr_task_new(&loop, NULL, sizeof(pair_frame), 0, sequential_mode ? sequential : overlap, clean);
        pxr_task_result result = pxr_loop_run(&loop);
        assert(result.status == PXR_TASK_OK && result.bits == 42 && cleaned == 3 && ran == 4);
        assert(clock.now == (sequential_mode ? 120 : 100) * 1000000);
        assert(clock.waits == 2 && event_count == 4);
        if (sequential_mode) assert(events[0] == 1 && events[1] == 11 && events[2] == 2 && events[3] == 12);
        else assert(events[0] == 1 && events[1] == 2 && events[2] == 12 && events[3] == 11);
        end_loop(&loop);
    }
}
static void implicit_join(pxr_task *task, void *raw) {
    (void)raw;
    pxr_task *a = new_timer(task, 1, 10), *b = new_timer(task, 2, 20);
    pxr_task_start(a); pxr_task_release(a); /* no detached task */
    pxr_task_start(b); /* scope will discard its unconsumed result */
    new_timer(task, 3, 1000000); /* never started */
    pxr_task *dropped = new_timer(task, 4, 1000000); pxr_task_release(dropped);
    pxr_task_finish(task, PXR_TAG_INT, 1);
}
static void parent_clean(pxr_task *task, void *raw) {
    assert(!task->pending_children && cleaned == 4);
    clean(task, raw);
}
static void test_scope(void) {
    pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock);
    cleaned = ran = event_count = 0;
    pxr_task_new(&loop, NULL, 0, 0, implicit_join, parent_clean);
    assert(pxr_loop_run(&loop).status == PXR_TASK_OK);
    assert(cleaned == 5 && ran == 4 && event_count == 4 && clock.now == 20000000);
    assert(loop.task_count == 1);
    end_loop(&loop);
}
static void fail(pxr_task *task, void *raw) { (void)raw; pxr_task_fail(task, EIO); }
static void failure_parent(pxr_task *task, void *raw) {
    (void)raw;
    pxr_task *slow = new_timer(task, 1, UINT64_MAX);
    pxr_task_start(slow);
    pxr_task *bad = pxr_task_new(task->loop, task, 0, 0, fail, clean);
    pxr_task_start(bad); pxr_task_release(bad); /* unobserved error must propagate */
    pxr_task_await(task, slow);
}
static void test_failure(void) {
    pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock);
    cleaned = ran = event_count = 0;
    pxr_task_new(&loop, NULL, 0, 0, failure_parent, clean);
    pxr_task_result result = pxr_loop_run(&loop);
    assert(result.status == PXR_TASK_ERROR && result.error == EIO);
    assert(cleaned == 3 && ran == 1 && clock.now == 0 && !clock.waits);
    end_loop(&loop);
}
static void yields(pxr_task *task, void *raw) {
    int *count = raw;
    if (++*count < 200) {
        pxr_task_yield(task);
        pxr_task_wake(task); pxr_task_wake(task); /* duplicate wakeups coalesce */
    } else pxr_task_finish(task, PXR_TAG_INT, *count);
}
static void cancel_self(pxr_task *task, void *raw) {
    (void)raw;
    pxr_task_start(new_timer(task, 1, UINT64_MAX));
    pxr_task_cancel(task);
}
static void test_cancellation(void) {
    pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock);
    cleaned = ran = event_count = 0;
    pxr_task *root = pxr_task_new(&loop, NULL, 0, 0, cancel_self, clean);
    assert(pxr_loop_run(&loop).status == PXR_TASK_CANCELLED);
    assert(cleaned == 2 && !clock.waits);
    end_loop(&loop);
    fake_init(&loop, &clock);
    root = pxr_task_new(&loop, NULL, sizeof(int), 0, yields, clean);
    pxr_task_cancel(root); /* cancellation before first resume */
    assert(pxr_loop_run(&loop).status == PXR_TASK_CANCELLED);
    end_loop(&loop);
    fake_init(&loop, &clock);
    pxr_task_new(&loop, NULL, sizeof(int), 0, yields, clean);
    assert(pxr_loop_run(&loop).bits == 200);
    end_loop(&loop);
}
static void timer_only(pxr_task *task, void *raw) {
    int *pc = raw;
    if (!(*pc)++) { pxr_task_sleep(task, 10); return; }
    pxr_task_finish(task, PXR_TAG_INT, 1);
}
static void test_wait_errors(void) {
    pxr_loop loop; fake_clock clock = { .interrupts = 3 }; fake_init(&loop, &clock);
    pxr_task_new(&loop, NULL, sizeof(int), 0, timer_only, clean);
    assert(pxr_loop_run(&loop).status == PXR_TASK_OK);
    assert(clock.now == 10000000 && clock.waits == 4);
    end_loop(&loop);
    clock = (fake_clock){ .fail = 1 }; fake_init(&loop, &clock);
    pxr_task_new(&loop, NULL, sizeof(int), 0, timer_only, clean);
    pxr_task_result result = pxr_loop_run(&loop);
    assert(result.status == PXR_TASK_ERROR && result.error == EIO);
    assert(loop.timer_count == 0);
    end_loop(&loop);
}

/* Actual nonblocking socket readiness, with short writes and EAGAIN retries.
 * The receiver waits before the sender's timer expires; a yielding task remains
 * runnable throughout. Descriptor cleanup runs on success, error and cancel. */
typedef struct { int fd, pc, send, eof; size_t offset; } socket_frame;
static size_t received;
static int would_block, sender_waited, progress;
static void socket_clean(pxr_task *task, void *raw) {
    socket_frame *frame = raw;
    if (frame->fd >= 0) { assert(close(frame->fd) == 0); frame->fd = -1; }
    clean(task, raw);
}
static void socket_task(pxr_task *task, void *raw) {
    socket_frame *frame = raw;
    if (!frame->pc++ && frame->send) { pxr_task_sleep(task, 2); return; }
    char buffer[4096]; memset(buffer, 'x', sizeof(buffer));
    if (frame->send) {
        while (frame->offset < 262144) {
            ssize_t n = send(frame->fd, buffer, sizeof(buffer), 0);
            if (n < 0 && errno == EINTR) continue;
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                sender_waited++; pxr_task_wait_fd(task, frame->fd, POLLOUT); return;
            }
            assert(n > 0); frame->offset += (size_t)n;
        }
        assert(shutdown(frame->fd, SHUT_WR) == 0);
        pxr_task_finish(task, PXR_TAG_INT, (int64_t)frame->offset);
    } else {
        for (;;) {
            ssize_t n = recv(frame->fd, buffer, sizeof(buffer), 0);
            if (n < 0 && errno == EINTR) continue;
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                would_block++; pxr_task_wait_fd(task, frame->fd, POLLIN); return;
            }
            assert(n >= 0);
            if (!n) { frame->eof = 1; pxr_task_finish(task, PXR_TAG_INT, (int64_t)received); return; }
            for (ssize_t i = 0; i < n; i++) assert(buffer[i] == 'x');
            received += (size_t)n;
        }
    }
}
static void progress_task(pxr_task *task, void *raw) {
    (void)raw; progress++;
    if (received < 262144) pxr_task_yield(task);
    else pxr_task_finish(task, PXR_TAG_INT, progress);
}
static void socket_parent(pxr_task *task, void *raw) {
    (void)raw;
    int sockets[2]; assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
    for (int i = 0; i < 2; i++) {
        assert(fcntl(sockets[i], F_SETFL, O_NONBLOCK) == 0);
        int size = 4096; assert(setsockopt(sockets[i], SOL_SOCKET, SO_SNDBUF, &size, sizeof(size)) == 0);
        pxr_task *child = pxr_task_new(task->loop, task, sizeof(socket_frame), 0, socket_task, socket_clean);
        socket_frame *frame = child->frame; frame->fd = sockets[i]; frame->send = i;
        pxr_task_start(child); pxr_task_release(child);
    }
    pxr_task *child = pxr_task_new(task->loop, task, 0, 0, progress_task, clean);
    pxr_task_start(child); pxr_task_release(child);
    pxr_task_finish(task, PXR_TAG_INT, 1);
}
static void test_sockets(void) {
    pxr_loop loop; pxr_loop_init(&loop); cleaned = 0;
    pxr_task_new(&loop, NULL, 0, 0, socket_parent, clean);
    assert(pxr_loop_run(&loop).status == PXR_TASK_OK);
    assert(received == 262144 && would_block && sender_waited && progress > 1 && cleaned == 4);
    end_loop(&loop);
}

/* Deep task trees and repeated awaited tasks exercise bounded control/frame
 * retention. All teardown paths are iterative, including failure propagation. */
static int maximum_depth, deep_error;
static void deep(pxr_task *task, void *raw) {
    int depth = *(int *)raw;
    if (depth < maximum_depth) {
        pxr_task *child = pxr_task_new(task->loop, task, sizeof(int), 0, deep, clean);
        *(int *)child->frame = depth + 1;
        pxr_task_start(child); pxr_task_release(child);
        pxr_task_finish(task, PXR_TAG_INT, depth);
    } else if (deep_error) pxr_task_fail(task, ERANGE);
    else pxr_task_finish(task, PXR_TAG_INT, depth);
}
static void test_deep(void) {
    maximum_depth = 2000;
    for (deep_error = 0; deep_error < 2; deep_error++) {
        pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock); cleaned = 0;
        pxr_task_new(&loop, NULL, sizeof(int), 0, deep, clean);
        pxr_task_result result = pxr_loop_run(&loop);
        assert(result.status == (deep_error ? PXR_TASK_ERROR : PXR_TASK_OK));
        assert(!deep_error || result.error == ERANGE);
        assert(cleaned == maximum_depth + 1 && loop.task_count == 1);
        end_loop(&loop);
    }
}

/* Repeated await/take frees control records and frames during the scope. */
typedef struct { int pc, count; pxr_task *child; } repeated_frame;
static void immediate(pxr_task *task, void *raw) {
    (void)raw;
    pxr_str *text = pxr_str_alloc(4096);
    text->data[0] = 'v'; text->data[4095] = 'z';
    pxr_task_finish(task, PXR_TAG_STR, (intptr_t)text);
}
static void repeated(pxr_task *task, void *raw) {
    repeated_frame *frame = raw;
    if (frame->pc) {
        pxr_task_result result = pxr_task_take(frame->child);
        pxr_str *text = (void *)(intptr_t)result.bits;
        assert(text->data[0] == 'v' && text->data[4095] == 'z');
        assert(task->loop->task_count == 1 && !task->children);
        assert(pxr_heap_bytes < 16384);
    }
    if (frame->count++ == 2000) { pxr_task_finish(task, PXR_TAG_INT, frame->count); return; }
    frame->pc = 1;
    frame->child = pxr_task_new(task->loop, task, 0, 0, immediate, clean);
    pxr_task_await(task, frame->child);
}
static void test_retention(void) {
    pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock);
    pxr_task_new(&loop, NULL, sizeof(repeated_frame), 0, repeated, clean);
    assert(pxr_loop_run(&loop).bits == 2001 && !clock.waits);
    end_loop(&loop);
}

static int cancel_fds[2], fd_cleaned;
static void waiting_socket(pxr_task *task, void *raw) {
    (void)raw; pxr_task_wait_fd(task, cancel_fds[0], POLLIN);
}
static void close_waiting_socket(pxr_task *task, void *raw) {
    assert(close(cancel_fds[0]) == 0); fd_cleaned++; clean(task, raw);
}
static void cancel_io(pxr_task *task, void *raw) {
    int *pc = raw;
    if (!(*pc)++) {
        pxr_task *child = pxr_task_new(task->loop, task, 0, 0, waiting_socket, close_waiting_socket);
        pxr_task_start(child); pxr_task_yield(task); return;
    }
    /* Child is now waiting for input that will never arrive. */
    assert(task->children->state == PXR_TASK_IO);
    pxr_task_cancel(task);
}
static void bad_io(pxr_task *task, void *raw) {
    int mode = *(int *)raw;
    if (mode == 0) pxr_task_wait_fd(task, -1, POLLIN);
    else if (mode == 1) pxr_task_wait_fd(task, cancel_fds[1], POLLIN); /* blocking */
    else if (mode == 2) pxr_task_wait_fd(task, cancel_fds[0], POLLPRI); /* unsupported event */
    else pxr_task_wait_fd(task, cancel_fds[0], POLLIN); /* already closed */
}
static void test_io_cleanup(void) {
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, cancel_fds) == 0);
    assert(fcntl(cancel_fds[0], F_SETFL, O_NONBLOCK) == 0);
    pxr_loop loop; pxr_loop_init(&loop);
    pxr_task_new(&loop, NULL, sizeof(int), 0, cancel_io, clean);
    assert(pxr_loop_run(&loop).status == PXR_TASK_CANCELLED && fd_cleaned == 1);
    end_loop(&loop);
    for (int mode = 0; mode < 4; mode++) {
        pxr_loop_init(&loop);
        pxr_task *root = pxr_task_new(&loop, NULL, sizeof(int), 0, bad_io, clean);
        *(int *)root->frame = mode;
        pxr_task_result result = pxr_loop_run(&loop);
        assert(result.status == PXR_TASK_ERROR && result.error == (mode == 3 ? EBADF : EINVAL));
        end_loop(&loop);
    }
    assert(close(cancel_fds[1]) == 0);
}

/* Randomized heap removals, equal deadlines, saturating overflow and clamping.
 * These checks are deterministic and require no wall-clock sleeps. */
static void never(pxr_task *task, void *raw) { (void)task; (void)raw; abort(); }
static void heap_driver(pxr_task *task, void *raw) {
    (void)raw;
    pxr_task *children[257];
    uint32_t random = 42;
    for (size_t i = 0; i < 257; i++) {
        random = random * 1664525 + 1013904223;
        children[i] = pxr_task_new(task->loop, task, 0, 0, never, clean);
        /* Simulate generated child frames reaching their first suspension. */
        task->loop->current = children[i]; children[i]->state = PXR_TASK_RUNNING;
        pxr_task_sleep(children[i], random % 13);
        task->loop->current = task;
    }
    task->loop->current = task;
    for (size_t i = 0; i < 257; i += 3) pxr_task_cancel(children[i]);
    uint64_t last_deadline = 0, last_id = 0;
    while (task->loop->timer_count) {
        pxr_task *next = task->loop->timers[0];
        assert(next->deadline > last_deadline || (next->deadline == last_deadline && next->id > last_id));
        last_deadline = next->deadline; last_id = next->id;
        pxr_task_cancel(next);
    }
    pxr_task_finish(task, PXR_TAG_INT, 1);
}
static int clamped_wait(struct pollfd *fds, nfds_t count, int timeout, void *context) {
    (void)fds; (void)context; assert(!count && timeout == INT_MAX);
    errno = EIO; return -1;
}
static void huge_timer(pxr_task *task, void *raw) {
    (void)raw; pxr_task_sleep(task, UINT64_MAX);
    assert(task->deadline == UINT64_MAX);
}
static void test_heap(void) {
    pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock);
    pxr_task_new(&loop, NULL, 0, 0, heap_driver, clean);
    assert(pxr_loop_run(&loop).status == PXR_TASK_OK && !clock.waits);
    end_loop(&loop);
    fake_init(&loop, &clock); loop.wait = clamped_wait;
    pxr_task_new(&loop, NULL, 0, 0, huge_timer, clean);
    assert(pxr_loop_run(&loop).error == EIO);
    end_loop(&loop);
}


typedef struct { pxr_object *object; } capture_frame;
static void capture_cleanup(pxr_task *task, void *raw) {
    capture_frame *frame = raw;
    assert(pxr_object_get_bits(frame->object, 0) == (intptr_t)frame->object);
    clean(task, raw);
}
static void lazy_capture(pxr_task *task, void *raw) {
    (void)raw;
    pxr_task *child = pxr_task_new(task->loop, task, sizeof(capture_frame), 1, never, capture_cleanup);
    capture_frame *frame = child->frame;
    frame->object = pxr_object_new(1, NULL, 1);
    pxr_object_set_bits(frame->object, 0, PXR_TAG_OBJ, (intptr_t)frame->object);
    child->roots[0] = frame->object; pxr_task_roots(child, 1);
    pxr_gc_collect(); /* A lazy frame owns its captures before it ever runs. */
    pxr_task_finish(task, PXR_TAG_INT, 0);
}
static void test_frame_roots(void) {
    pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock);
    pxr_task_new(&loop, NULL, 0, 0, lazy_capture, clean);
    assert(pxr_loop_run(&loop).status == PXR_TASK_OK);
    end_loop(&loop);
    fake_init(&loop, &clock);
    pxr_task *root = pxr_task_new(&loop, NULL, 0, 0, immediate, clean);
    pxr_task_result result = pxr_loop_run(&loop);
    assert(!root->frame && !root->roots);
    pxr_gc_collect();
    pxr_str *text = (void *)(intptr_t)result.bits;
    assert(text->len == 4096 && text->data[0] == 'v' && text->data[4095] == 'z');
    end_loop(&loop);
}
static void specific_fail(pxr_task *task, void *raw) { pxr_task_fail(task, *(int *)raw); }
static void two_failures(pxr_task *task, void *raw) {
    (void)raw;
    pxr_task *a = pxr_task_new(task->loop, task, sizeof(int), 0, specific_fail, clean);
    pxr_task *b = pxr_task_new(task->loop, task, sizeof(int), 0, specific_fail, clean);
    *(int *)a->frame = ERANGE; *(int *)b->frame = EIO;
    pxr_task_start(b); pxr_task_start(a); /* execution order differs from creation */
    pxr_task_finish(task, PXR_TAG_INT, 1);
}
static void test_failure_order(void) {
    pxr_loop loop; fake_clock clock = {0}; fake_init(&loop, &clock);
    pxr_task_new(&loop, NULL, 0, 0, two_failures, clean);
    pxr_task_result result = pxr_loop_run(&loop);
    assert(result.status == PXR_TASK_ERROR && result.error == EIO);
    end_loop(&loop);
}

int main(void) {
    alarm(20); /* Turn a scheduler regression into a bounded test failure. */
    test_timers(); test_scope(); test_failure(); test_cancellation();
    test_wait_errors(); test_sockets(); test_deep();
    test_retention(); test_io_cleanup(); test_heap();
    test_frame_roots(); test_failure_order();
    alarm(0);
    puts("async clean");
}
