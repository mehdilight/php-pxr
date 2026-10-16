/* PXR single-thread async runtime, appended to pxr_rt.c by the driver.
 * Internal ABI for compiler-generated resumable frames and owned operations.
 * See docs/ASYNC_RUNTIME.md for ownership and failure rules.
 * Original implementation; upstream reading is recorded in ASYNC_REFERENCES.md.
 */
#include <assert.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <time.h>

typedef struct pxr_task pxr_task;
typedef struct pxr_loop pxr_loop;
typedef struct {
    pxr_array *globals[6], *headers, *response_headers;
    pxr_str *body;
    size_t scan, header_end, body_length;
    int parsed, head, status, error, chunked, chunk_state, continue_pending;
    size_t chunk_remaining, chunk_count, chunk_metadata, chunk_scan, trailer_count, trailer_bytes;
    int64_t max_bytes;
    pxr_buf output, decoded;
} pxr_http_context;
static pxr_http_context *pxr_http_cli_context;
static void pxr_http_mark_context(pxr_http_context *context);
static pxr_http_context *pxr_http_context_new(int fd, int64_t max_bytes);
typedef void (*pxr_task_callback)(pxr_task *, void *);
typedef struct { int status, error; int64_t tag, bits; } pxr_task_result;
enum { PXR_TASK_OK, PXR_TASK_CANCELLED, PXR_TASK_ERROR };
enum { PXR_TASK_NEW, PXR_TASK_READY, PXR_TASK_RUNNING, PXR_TASK_TIMER,
       PXR_TASK_IO, PXR_TASK_CHILD, PXR_TASK_CHILDREN, PXR_TASK_JOINING, PXR_TASK_COMPLETE };

struct pxr_task {
    pxr_loop *loop;
    pxr_task *all_prev, *all_next, *ready_next;
    pxr_task *parent, *children, *sibling_prev, *sibling_next, *waiting;
    size_t pending_children;
    uint64_t id, deadline;
    size_t timer_index;
    int state, queued, handle_live, claimed, body_done, cancel_requested;
    int fd; short io_events, io_ready;
    pxr_task_result result;
    pxr_task_callback resume, cleanup;
    void *frame;
    void **roots;
    size_t root_count, root_capacity;
    pxr_http_context *http;
};
struct pxr_loop {
    pxr_loop *registered_next;
    pxr_task *tasks, *ready_head, *ready_tail, *current, *root;
    pxr_task **timers;
    size_t timer_count, timer_capacity, task_count;
    struct pollfd *poll_fds;
    pxr_task **poll_tasks;
    size_t poll_capacity;
    uint64_t next_id;
    int running;
    uint64_t (*now)(void *);
    int (*wait)(struct pollfd *, nfds_t, int, void *);
    void *clock_context;
};
static pxr_loop *pxr_async_loops;
static int pxr_async_running;

static void *pxr_async_resize(void *ptr, size_t count, size_t size) {
    void *result = realloc(ptr, pxr_size_mul(count, size));
    if (!result) pxr_oom();
    return result;
}
static uint64_t pxr_async_now(void *context) {
    (void)context;
    struct timespec time;
    if (clock_gettime(CLOCK_MONOTONIC, &time) != 0) abort();
    uint64_t seconds = (uint64_t)time.tv_sec;
    if (seconds > (UINT64_MAX - (uint64_t)time.tv_nsec) / 1000000000)
        return UINT64_MAX;
    return seconds * 1000000000 + (uint64_t)time.tv_nsec;
}
static int pxr_async_wait(struct pollfd *fds, nfds_t count, int timeout, void *context) {
    (void)context;
    return poll(fds, count, timeout);
}
static void pxr_loop_init(pxr_loop *loop) {
    memset(loop, 0, sizeof(*loop));
    loop->now = pxr_async_now;
    loop->wait = pxr_async_wait;
    loop->registered_next = pxr_async_loops;
    pxr_async_loops = loop;
}
static void pxr_async_mark_roots(void) {
    pxr_http_mark_context(pxr_http_cli_context);
    for (pxr_loop *loop = pxr_async_loops; loop; loop = loop->registered_next)
        for (pxr_task *task = loop->tasks; task; task = task->all_next) {
            pxr_gc_mark(task->frame);
            for (size_t i = 0; i < task->root_count; i++) pxr_gc_mark(task->roots[i]);
            pxr_gc_mark_value(task->result.tag, task->result.bits);
            pxr_http_mark_context(task->http);
        }
}
/* Roots are exact payload addresses. A frame is raw storage, never scanned.
 * Publish all pointer fields before any operation that can collect, including
 * cleanup callbacks. Table entries may be updated directly by generated code. */
static void pxr_task_roots(pxr_task *task, size_t count) {
    assert(count <= task->root_capacity);
    task->root_count = count;
}
static pxr_task *pxr_task_new(pxr_loop *loop, pxr_task *parent,
                              size_t frame_size, size_t root_capacity,
                              pxr_task_callback resume, pxr_task_callback cleanup) {
    assert(resume && (!parent || (parent == loop->current &&
           parent->state == PXR_TASK_RUNNING && !parent->body_done)));
    assert(parent || (!loop->root && !loop->running));
    pxr_task *task = calloc(1, sizeof(*task));
    if (!task) pxr_oom();
    task->loop = loop;
    if (loop->next_id == UINT64_MAX) pxr_oom();
    task->id = ++loop->next_id;
    task->timer_index = SIZE_MAX;
    task->fd = -1;
    task->handle_live = 1;
    task->resume = resume;
    task->cleanup = cleanup;
    task->frame = pxr_heap_alloc(frame_size);
    memset(task->frame, 0, frame_size);
    if (root_capacity) {
        task->roots = calloc(root_capacity, sizeof(void *));
        if (!task->roots) pxr_oom();
    }
    task->root_capacity = root_capacity;
    task->all_next = loop->tasks;
    if (loop->tasks) loop->tasks->all_prev = task;
    loop->tasks = task;
    loop->task_count++;
    task->parent = parent;
    task->http = parent ? parent->http : NULL;
    if (parent) {
        task->sibling_next = parent->children;
        if (parent->children) parent->children->sibling_prev = task;
        parent->children = task;
        parent->pending_children++;
    } else loop->root = task;
    return task;
}

/* Indexed timer heap: cancellation removes a long timer immediately. */
static int pxr_timer_less(pxr_task *a, pxr_task *b) {
    return a->deadline < b->deadline || (a->deadline == b->deadline && a->id < b->id);
}
static void pxr_timer_swap(pxr_loop *loop, size_t a, size_t b) {
    pxr_task *temp = loop->timers[a];
    loop->timers[a] = loop->timers[b]; loop->timers[b] = temp;
    loop->timers[a]->timer_index = a; loop->timers[b]->timer_index = b;
}
static void pxr_timer_up(pxr_loop *loop, size_t i) {
    while (i && pxr_timer_less(loop->timers[i], loop->timers[(i - 1) / 2])) {
        size_t parent = (i - 1) / 2;
        pxr_timer_swap(loop, i, parent); i = parent;
    }
}
static void pxr_timer_down(pxr_loop *loop, size_t i) {
    while (i < loop->timer_count / 2) {
        size_t child = i * 2 + 1;
        if (child + 1 < loop->timer_count && pxr_timer_less(loop->timers[child + 1], loop->timers[child])) child++;
        if (!pxr_timer_less(loop->timers[child], loop->timers[i])) break;
        pxr_timer_swap(loop, i, child); i = child;
    }
}
static void pxr_timer_remove(pxr_task *task) {
    if (task->timer_index == SIZE_MAX) return;
    pxr_loop *loop = task->loop;
    size_t i = task->timer_index, last = --loop->timer_count;
    task->timer_index = SIZE_MAX;
    if (i == last) return;
    loop->timers[i] = loop->timers[last]; loop->timers[i]->timer_index = i;
    if (i && pxr_timer_less(loop->timers[i], loop->timers[(i - 1) / 2])) pxr_timer_up(loop, i);
    else pxr_timer_down(loop, i);
}
static void pxr_task_wake(pxr_task *task) {
    if (task->state == PXR_TASK_COMPLETE || task->queued) return;
    pxr_timer_remove(task);
    task->state = PXR_TASK_READY;
    task->queued = 1;
    task->ready_next = NULL;
    pxr_loop *loop = task->loop;
    if (loop->ready_tail) loop->ready_tail->ready_next = task;
    else loop->ready_head = task;
    loop->ready_tail = task;
}
static void pxr_task_start(pxr_task *task) {
    assert(task->state == PXR_TASK_NEW && task->handle_live);
    assert(!task->parent || task->parent == task->loop->current);
    pxr_task_wake(task);
}
static void pxr_task_cancel(pxr_task *task) {
    if (task->state == PXR_TASK_COMPLETE || task->cancel_requested) return;
    task->cancel_requested = 1;
    pxr_task_wake(task);
}
static void pxr_task_destroy(pxr_task *task) {
    assert(task->state == PXR_TASK_COMPLETE && !task->queued && !task->children);
    pxr_loop *loop = task->loop;
    if (task->sibling_prev) task->sibling_prev->sibling_next = task->sibling_next;
    else if (task->parent) task->parent->children = task->sibling_next;
    if (task->sibling_next) task->sibling_next->sibling_prev = task->sibling_prev;
    if (task->all_prev) task->all_prev->all_next = task->all_next;
    else loop->tasks = task->all_next;
    if (task->all_next) task->all_next->all_prev = task->all_prev;
    if (loop->root == task) loop->root = NULL;
    loop->task_count--;
    free(task);
}
/* Dropping a started handle does not detach or cancel its child. Scope exit
 * still joins it and propagates errors. Dropping a lazy handle never runs it. */
static void pxr_task_release(pxr_task *task) {
    assert(task->handle_live);
    assert(task->parent || task->state == PXR_TASK_COMPLETE);
    task->handle_live = 0;
    if (task->state == PXR_TASK_COMPLETE) pxr_task_destroy(task);
    else if (task->state == PXR_TASK_NEW) pxr_task_cancel(task);
}
static void pxr_task_yield(pxr_task *task) {
    assert(task == task->loop->current && task->state == PXR_TASK_RUNNING);
    pxr_task_wake(task);
}
static void pxr_task_timer_at(pxr_task *task, uint64_t deadline) {
    assert(task->timer_index == SIZE_MAX);
    pxr_loop *loop = task->loop;
    task->deadline = deadline;
    if (loop->timer_count == loop->timer_capacity) {
        loop->timer_capacity = pxr_grow_capacity(loop->timer_capacity, pxr_size_add(loop->timer_count, 1));
        loop->timers = pxr_async_resize(loop->timers, loop->timer_capacity, sizeof(*loop->timers));
    }
    size_t i = loop->timer_count++;
    loop->timers[i] = task; task->timer_index = i;
    pxr_timer_up(loop, i);
}
static void pxr_task_sleep(pxr_task *task, uint64_t milliseconds) {
    assert(task == task->loop->current && task->state == PXR_TASK_RUNNING);
    uint64_t now = task->loop->now(task->loop->clock_context);
    uint64_t deadline = milliseconds > (UINT64_MAX - now) / 1000000
        ? UINT64_MAX : now + milliseconds * 1000000;
    task->state = PXR_TASK_TIMER;
    pxr_task_timer_at(task, deadline);
}
static void pxr_task_finish(pxr_task *task, int64_t tag, int64_t bits) {
    assert(task == task->loop->current && task->state == PXR_TASK_RUNNING);
    task->result.tag = tag; task->result.bits = bits;
    task->body_done = 1;
    pxr_task_wake(task);
}
static void pxr_task_fail(pxr_task *task, int error) {
    assert(error > 0 && task->state != PXR_TASK_COMPLETE);
    if (!task->result.error) task->result.error = error;
    task->result.status = PXR_TASK_ERROR;
    task->body_done = 1;
    pxr_task_wake(task);
}
static void pxr_task_wait_fd(pxr_task *task, int fd, short events) {
    assert(task == task->loop->current && task->state == PXR_TASK_RUNNING);
    if (fd < 0 || !events || (events & ~(POLLIN | POLLOUT))) {
        pxr_task_fail(task, EINVAL); return;
    }
    int flags = fcntl(fd, F_GETFL);
    if (flags < 0) { pxr_task_fail(task, errno); return; }
    if (!(flags & O_NONBLOCK)) { pxr_task_fail(task, EINVAL); return; }
    task->fd = fd; task->io_events = events; task->io_ready = 0;
    task->state = PXR_TASK_IO;
}
static void pxr_task_await(pxr_task *parent, pxr_task *child) {
    assert(parent == parent->loop->current && parent->state == PXR_TASK_RUNNING);
    assert(child->parent == parent && child->handle_live && !child->claimed);
    child->claimed = 1;
    if (child->state == PXR_TASK_COMPLETE) { pxr_task_yield(parent); return; }
    if (child->state == PXR_TASK_NEW) pxr_task_start(child);
    parent->waiting = child;
    parent->state = PXR_TASK_CHILD;
}
static pxr_task_result pxr_task_take(pxr_task *child) {
    assert(child->parent == child->loop->current && child->claimed && child->handle_live);
    assert(child->state == PXR_TASK_COMPLETE);
    pxr_task_result result = child->result;
    pxr_task_release(child);
    /* No collection until the caller publishes any returned pointer. */
    return result;
}

/* Completion and cancellation travel through the queue, never recursive task
 * callbacks. Deep task trees cannot overflow the native stack during cleanup. */
static void pxr_task_step(pxr_task *task) {
    pxr_loop *loop = task->loop;
    if (task->cancel_requested) {
        task->body_done = 1;
        if (!task->result.error) task->result.status = PXR_TASK_CANCELLED;
    }
    if (!task->body_done) {
        task->state = PXR_TASK_RUNNING;
        loop->current = task;
        task->resume(task, task->frame);
        loop->current = NULL;
        assert(task->state != PXR_TASK_RUNNING); /* callback must suspend/finish */
        return;
    }
    task->state = PXR_TASK_JOINING;
    for (pxr_task *child = task->children; child; child = child->sibling_next)
        if (child->state != PXR_TASK_COMPLETE &&
            (task->result.status != PXR_TASK_OK || child->state == PXR_TASK_NEW))
            pxr_task_cancel(child);
    if (task->pending_children) return;
    /* Children are complete before the parent's resources/locals are released.
     * Cleanup may collect; this task and its roots stay registered throughout. */
    if (task->cleanup) task->cleanup(task, task->frame);
    while (task->children) pxr_task_destroy(task->children);
    pxr_heap_free(task->frame); task->frame = NULL;
    free(task->roots); task->roots = NULL; task->root_count = 0;
    task->state = PXR_TASK_COMPLETE;
    pxr_task *parent = task->parent;
    if (parent) {
        assert(parent->pending_children);
        parent->pending_children--;
        if (task->result.status == PXR_TASK_ERROR) {
            if (!parent->result.error) parent->result.error = task->result.error;
            parent->result.status = PXR_TASK_ERROR;
            parent->body_done = 1;
        }
        if (parent->waiting == task) {
            parent->waiting = NULL;
            pxr_task_wake(parent);
        }
        if (parent->state == PXR_TASK_CHILDREN) pxr_task_wake(parent);
        if (parent->body_done) pxr_task_wake(parent);
    }
    if (!task->handle_live) pxr_task_destroy(task);
}
static void pxr_loop_events(pxr_loop *loop) {
    uint64_t now = loop->now(loop->clock_context);
    while (loop->timer_count && loop->timers[0]->deadline <= now)
        pxr_task_wake(loop->timers[0]);
    size_t count = 0;
    for (pxr_task *task = loop->tasks; task; task = task->all_next)
        if (task->state == PXR_TASK_IO) count++;
    if (count > loop->poll_capacity) {
        loop->poll_capacity = pxr_grow_capacity(loop->poll_capacity, count);
        loop->poll_fds = pxr_async_resize(loop->poll_fds, loop->poll_capacity, sizeof(*loop->poll_fds));
        loop->poll_tasks = pxr_async_resize(loop->poll_tasks, loop->poll_capacity, sizeof(*loop->poll_tasks));
    }
    size_t i = 0;
    for (pxr_task *task = loop->tasks; task; task = task->all_next)
        if (task->state == PXR_TASK_IO) {
            loop->poll_fds[i] = (struct pollfd){ task->fd, task->io_events, 0 };
            loop->poll_tasks[i++] = task;
        }
    int timeout = -1;
    if (loop->ready_head) timeout = 0;
    else if (loop->timer_count) {
        uint64_t delta = loop->timers[0]->deadline - now;
        uint64_t ms = delta / 1000000 + (delta % 1000000 != 0);
        timeout = ms > INT_MAX ? INT_MAX : (int)ms;
    } else if (!count) {
        pxr_task_fail(loop->root, EDEADLK); return;
    }
    if (!count && !timeout) return;
    if ((nfds_t)count != count) pxr_oom();
    int ready = loop->wait(loop->poll_fds, (nfds_t)count, timeout, loop->clock_context);
    if (ready < 0) {
        if (errno != EINTR) pxr_task_fail(loop->root, errno);
        return; /* Recompute deadlines even after EINTR. */
    }
    for (i = 0; i < count; i++) {
        short events = loop->poll_fds[i].revents;
        pxr_task *task = loop->poll_tasks[i];
        if (events & POLLNVAL) pxr_task_fail(task, EBADF);
        else if (events) { task->io_ready = events; pxr_task_wake(task); }
    }
}
static pxr_task_result pxr_loop_run(pxr_loop *loop) {
    assert(loop->root && !loop->running && !pxr_async_running);
    loop->running = pxr_async_running = 1;
    if (loop->root->state == PXR_TASK_NEW) pxr_task_start(loop->root);
    while (loop->root->state != PXR_TASK_COMPLETE) {
        /* Poll I/O after a bounded ready batch so self-yielding CPU work cannot
         * starve timers or already-ready sockets. No busy polling when idle. */
        for (unsigned budget = 0; budget < 64 && loop->ready_head; budget++) {
            pxr_task *task = loop->ready_head;
            loop->ready_head = task->ready_next;
            if (!loop->ready_head) loop->ready_tail = NULL;
            task->queued = 0; task->ready_next = NULL;
            pxr_task_step(task);
            pxr_gc_safepoint();
        }
        if (loop->root->state != PXR_TASK_COMPLETE) pxr_loop_events(loop);
    }
    loop->running = pxr_async_running = 0;
    return loop->root->result;
}
static void pxr_loop_destroy(pxr_loop *loop) {
    assert(!loop->running);
    if (loop->root) {
        assert(loop->root->state == PXR_TASK_COMPLETE);
        pxr_task_release(loop->root);
    }
    assert(!loop->tasks && !loop->task_count && !loop->timer_count && !loop->ready_head);
    pxr_loop **link = &pxr_async_loops;
    while (*link != loop) { assert(*link); link = &(*link)->registered_next; }
    *link = loop->registered_next;
    free(loop->timers); free(loop->poll_fds); free(loop->poll_tasks);
    memset(loop, 0, sizeof(*loop));
}

/* Generated-code bridge. Cells are 64-bit ABI payloads; a pointer bitmap
 * atomically replaces the frame's live root view. Storing a new pointer roots
 * it immediately, before a following generated safepoint can collect it. */
static pxr_loop pxr_compiler_loop;
static int pxr_compiler_loop_initialized;
void *pxr_async_create(pxr_task_callback callback, int64_t cells) {
    assert(cells > 0);
    if (!pxr_compiler_loop_initialized) {
        pxr_loop_init(&pxr_compiler_loop);
        pxr_compiler_loop_initialized = 1;
    }
    pxr_task *task = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current,
        pxr_size_mul((size_t)cells, sizeof(int64_t)), (size_t)cells, callback, NULL);
    pxr_task_roots(task, (size_t)cells);
    return task;
}
int64_t pxr_async_get(void *handle, int64_t cell) {
    pxr_task *task = handle;
    assert(task->frame && cell >= 0 && (uint64_t)cell < task->root_capacity);
    return ((int64_t *)task->frame)[cell];
}
void pxr_async_set(void *handle, int64_t cell, int64_t bits, int64_t pointer) {
    pxr_task *task = handle;
    assert(task->frame && cell >= 0 && (uint64_t)cell < task->root_capacity);
    ((int64_t *)task->frame)[cell] = bits;
    task->roots[cell] = pointer ? (void *)(intptr_t)bits : NULL;
}
void pxr_async_publish(void *handle, pxr_str *mask) {
    pxr_task *task = handle;
    assert(mask->len == task->root_capacity / 8 + (task->root_capacity % 8 != 0));
    for (size_t i = 0; i < task->root_capacity; i++)
        task->roots[i] = (mask->data[i / 8] & (1u << (i % 8)))
            ? (void *)(intptr_t)((int64_t *)task->frame)[i] : NULL;
}
void pxr_async_await(void *parent, void *child) { pxr_task_await(parent, child); }
int64_t pxr_async_take(void *child) {
    pxr_task_result result = pxr_task_take(child);
    assert(result.status == PXR_TASK_OK);
    return result.bits;
}
void pxr_async_finish(void *task, int64_t tag, int64_t bits) { pxr_task_finish(task, tag, bits); }
void *pxr_async_spawn(void *handle) {
    pxr_task *task = handle;
    assert(task->parent == task->loop->current && !task->claimed);
    pxr_task_start(task);
    return task;
}
typedef struct { int pc; uint64_t delay; } pxr_async_timer_frame;
static void pxr_async_timer_resume(pxr_task *task, void *raw) {
    pxr_async_timer_frame *frame = raw;
    if (frame->pc < 0) { pxr_task_fail(task, EINVAL); return; }
    if (!frame->pc++) pxr_task_sleep(task, frame->delay);
    else pxr_task_finish(task, 0, 0);
}
void *pxr_async_sleep_task(int64_t milliseconds) {
    assert(pxr_compiler_loop_initialized && pxr_compiler_loop.current);
    pxr_task *task = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current,
        sizeof(pxr_async_timer_frame), 0, pxr_async_timer_resume, NULL);
    if (milliseconds < 0) {
        /* Preserve laziness: report the invalid duration when the task starts. */
        ((pxr_async_timer_frame *)task->frame)->pc = -1;
    } else ((pxr_async_timer_frame *)task->frame)->delay = (uint64_t)milliseconds;
    return task;
}
void pxr_async_run(void *handle) {
    assert(pxr_compiler_loop_initialized && pxr_compiler_loop.root == handle);
    pxr_task_result result = pxr_loop_run(&pxr_compiler_loop);
    pxr_loop_destroy(&pxr_compiler_loop);
    pxr_compiler_loop_initialized = 0;
    if (result.status != PXR_TASK_OK) {
        fprintf(stderr, "pxr: async task failed: %s\n",
            result.status == PXR_TASK_CANCELLED ? "cancelled" : strerror(result.error));
        exit(255);
    }
}
void pxr_async_drop(void *task) { pxr_task_release(task); }

/* Consume a handle without detaching its task. Parents still join cancellation
 * cleanup; cancellation cannot overwrite an already observed task error. */
void pxr_async_cancel(void *handle) {
    pxr_task *task = handle;
    assert(task->parent == task->loop->current && task->handle_live && !task->claimed);
    pxr_task_cancel(task);
    pxr_task_release(task);
}

/* One owned TCP connection per exchange. No descriptor crosses the language
 * ABI. Numeric addresses keep name resolution out of the executor thread. */
#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>

enum { PXR_TCP_NEW, PXR_TCP_CONNECTING, PXR_TCP_SENDING, PXR_TCP_READING };
typedef struct {
    int fd, phase, started;
    int64_t port, max_bytes, timeout_ms;
    uint64_t expires;
    pxr_str *address, *request;
    size_t sent;
    pxr_buf response;
} pxr_tcp_frame;

static void pxr_tcp_cleanup(pxr_task *task, void *raw) {
    pxr_tcp_frame *frame = raw;
    if (frame->fd >= 0) {
        int fd = frame->fd;
        frame->fd = -1;
        (void)close(fd); /* Never retry close: the fd may already be released. */
    }
    task->roots[2] = NULL;
    pxr_heap_free(frame->response.data);
    frame->response.data = NULL;
}
static int pxr_tcp_address(pxr_str *text, int64_t port,
                            struct sockaddr_storage *storage, socklen_t *length) {
    if (port < 1 || port > 65535 || !text->len || text->len >= INET6_ADDRSTRLEN ||
        memchr(text->data, 0, text->len)) return EINVAL;
    char address[INET6_ADDRSTRLEN];
    memcpy(address, text->data, text->len);
    address[text->len] = 0;
    struct sockaddr_in v4 = {0};
    struct sockaddr_in6 v6 = {0};
    if (inet_pton(AF_INET, address, &v4.sin_addr) == 1) {
        v4.sin_family = AF_INET;
        v4.sin_port = htons((uint16_t)port);
        *length = sizeof(v4); memcpy(storage, &v4, sizeof(v4));
    } else if (inet_pton(AF_INET6, address, &v6.sin6_addr) == 1) {
        v6.sin6_family = AF_INET6;
        v6.sin6_port = htons((uint16_t)port);
        *length = sizeof(v6); memcpy(storage, &v6, sizeof(v6));
    } else return EINVAL;
    return 0;
}
static int pxr_tcp_configure(int fd) {
    int flags = fcntl(fd, F_GETFL);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) return errno;
    flags = fcntl(fd, F_GETFD);
    if (flags < 0 || fcntl(fd, F_SETFD, flags | FD_CLOEXEC) < 0) return errno;
#ifdef SO_NOSIGPIPE
    int yes = 1;
    if (setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &yes, sizeof(yes)) < 0) return errno;
#elif !defined(MSG_NOSIGNAL)
    return ENOTSUP; /* Do not change the process-wide SIGPIPE disposition. */
#endif
    return 0;
}
static int pxr_tcp_open(pxr_tcp_frame *frame, struct sockaddr_storage *storage,
                        socklen_t *length) {
    if (frame->max_bytes < 0) return EINVAL;
    int error = pxr_tcp_address(frame->address, frame->port, storage, length);
    if (error) return error;
    frame->fd = socket(((struct sockaddr *)storage)->sa_family, SOCK_STREAM, 0);
    if (frame->fd < 0) return errno;
    return pxr_tcp_configure(frame->fd);
}
static void pxr_tcp_wait(pxr_task *task, pxr_tcp_frame *frame, short events) {
    pxr_task_wait_fd(task, frame->fd, events);
    if (task->state == PXR_TASK_IO) pxr_task_timer_at(task, frame->expires);
}
static void pxr_tcp_resume(pxr_task *task, void *raw) {
    pxr_tcp_frame *frame = raw;
    uint64_t now = task->loop->now(task->loop->clock_context);
    if (!frame->started) {
        if (frame->timeout_ms < 0) { pxr_task_fail(task, EINVAL); return; }
        frame->expires = (uint64_t)frame->timeout_ms > (UINT64_MAX - now) / 1000000
            ? UINT64_MAX : now + (uint64_t)frame->timeout_ms * 1000000;
        frame->started = 1;
    }
    if (now >= frame->expires) { pxr_task_fail(task, ETIMEDOUT); return; }
    if (frame->phase == PXR_TCP_NEW) {
        struct sockaddr_storage address;
        socklen_t length;
        int error = pxr_tcp_open(frame, &address, &length);
        if (error) { pxr_task_fail(task, error); return; }
        int result = connect(frame->fd, (struct sockaddr *)&address, length);
        if (result < 0 && errno != EINPROGRESS && errno != EINTR) {
            pxr_task_fail(task, errno); return;
        }
        if (result < 0) {
            frame->phase = PXR_TCP_CONNECTING;
            pxr_tcp_wait(task, frame, POLLOUT); return;
        }
        frame->phase = PXR_TCP_SENDING;
    }
    if (frame->phase == PXR_TCP_CONNECTING) {
        int error = 0; socklen_t length = sizeof(error);
        if (getsockopt(frame->fd, SOL_SOCKET, SO_ERROR, &error, &length) < 0) {
            if (errno == EINTR) pxr_task_yield(task);
            else pxr_task_fail(task, errno);
            return;
        }
        if (error == EINPROGRESS || error == EALREADY) {
            pxr_tcp_wait(task, frame, POLLOUT); return;
        }
        if (error) { pxr_task_fail(task, error); return; }
        frame->phase = PXR_TCP_SENDING;
    }
    /* Bound each callback even when the peer continuously accepts/sends data. */
    for (unsigned budget = 0; budget < 64; budget++) {
        if (frame->phase == PXR_TCP_SENDING) {
            if (frame->sent == frame->request->len) {
                if (shutdown(frame->fd, SHUT_WR) < 0) {
                    if (errno == EINTR) { pxr_task_yield(task); return; }
                    pxr_task_fail(task, errno); return;
                }
                frame->phase = PXR_TCP_READING;
                continue;
            }
            size_t size = frame->request->len - frame->sent;
            if (size > 8192) size = 8192;
            int flags = 0;
#ifdef MSG_NOSIGNAL
            flags = MSG_NOSIGNAL;
#endif
            ssize_t n = send(frame->fd, frame->request->data + frame->sent, size, flags);
            if (n < 0 && errno == EINTR) continue;
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                pxr_tcp_wait(task, frame, POLLOUT); return;
            }
            if (n <= 0) { pxr_task_fail(task, n < 0 ? errno : EPIPE); return; }
            frame->sent += (size_t)n;
        } else {
            char chunk[8192];
            ssize_t n = recv(frame->fd, chunk, sizeof(chunk), 0);
            if (n < 0 && errno == EINTR) continue;
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                pxr_tcp_wait(task, frame, POLLIN); return;
            }
            if (n < 0) { pxr_task_fail(task, errno); return; }
            if (!n) {
                pxr_str *result = pxr_str_alloc(frame->response.len);
                if (frame->response.len) memcpy(result->data, frame->response.data, frame->response.len);
                pxr_task_finish(task, PXR_TAG_STR, (intptr_t)result); return;
            }
            if ((size_t)n > (size_t)frame->max_bytes - frame->response.len) {
                pxr_task_fail(task, EMSGSIZE); return;
            }
            pxr_buf_append(&frame->response, chunk, (size_t)n);
            task->roots[2] = frame->response.data;
        }
    }
    pxr_task_yield(task);
}
void *pxr_async_tcp_exchange(pxr_str *address, int64_t port,
                             pxr_str *request, int64_t max_bytes, int64_t timeout_ms) {
    assert(pxr_compiler_loop_initialized && pxr_compiler_loop.current);
    pxr_task *task = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current,
        sizeof(pxr_tcp_frame), 3, pxr_tcp_resume, pxr_tcp_cleanup);
    pxr_tcp_frame *frame = task->frame;
    frame->fd = -1; frame->address = address; frame->port = port;
    frame->request = request; frame->max_bytes = max_bytes; frame->timeout_ms = timeout_ms;
    task->roots[0] = address; task->roots[1] = request;
    pxr_task_roots(task, 3);
    return task;
}

/* Bounded request/response server. A literal source handler is compiled to a
 * constructor pointer; descriptors never escape the structured connection task.
 * EOF delimits each request, exactly as in tcpExchangeAsync. */
typedef pxr_task *(*pxr_tcp_handler)(pxr_str *);
typedef pxr_task *(*pxr_http_handler)(void);
typedef struct {
    int fd, started;
    pxr_str *address;
    pxr_tcp_handler handler;
    pxr_http_handler http_handler;
    int64_t port, max_requests, remaining, max_bytes, timeout_ms, max_concurrent;
} pxr_server_frame;
enum { PXR_SERVER_READ, PXR_SERVER_HANDLER, PXR_SERVER_WRITE, PXR_SERVER_CONTINUE,
       PXR_SERVER_SHUTDOWN, PXR_SERVER_DRAIN };
typedef struct {
    int fd, phase;
    int64_t max_bytes;
    uint64_t expires;
    pxr_tcp_handler handler;
    pxr_http_handler http_handler;
    pxr_task *handler_task;
    pxr_buf request;
    pxr_str *response;
    size_t sent;
} pxr_server_connection;
static int pxr_http_consume(pxr_task *task, pxr_server_connection *frame, int eof);
static void pxr_http_response(pxr_task *task, pxr_server_connection *frame, pxr_str *body, int status);

static void pxr_server_close(int *slot) {
    if (*slot < 0) return;
    int fd = *slot; *slot = -1;
    (void)close(fd); /* Never retry close, including EINTR. */
}
static void pxr_server_cleanup(pxr_task *task, void *raw) {
    (void)task;
    pxr_server_close(&((pxr_server_frame *)raw)->fd);
}
static void pxr_server_connection_cleanup(pxr_task *task, void *raw) {
    pxr_server_connection *frame = raw;
    pxr_server_close(&frame->fd);
    task->roots[0] = NULL;
    pxr_heap_free(frame->request.data); frame->request.data = NULL;
}
static void pxr_server_wait(pxr_task *task, pxr_server_connection *frame, short events) {
    pxr_task_wait_fd(task, frame->fd, events);
    if (task->state == PXR_TASK_IO) pxr_task_timer_at(task, frame->expires);
}
static void pxr_server_connection_resume(pxr_task *task, void *raw) {
    pxr_server_connection *frame = raw;
    if (task->loop->now(task->loop->clock_context) >= frame->expires) {
        if (frame->phase >= PXR_SERVER_SHUTDOWN) pxr_task_finish(task, 0, 0);
        else pxr_task_fail(task, ETIMEDOUT);
        return;
    }
    if (frame->phase == PXR_SERVER_HANDLER) {
        pxr_task_result result = pxr_task_take(frame->handler_task);
        assert(result.status == PXR_TASK_OK && (result.tag == PXR_TAG_STR || (frame->http_handler && result.tag == 0)));
        frame->handler_task = NULL;
        frame->response = result.tag == PXR_TAG_STR ? (void *)(intptr_t)result.bits : pxr_str_alloc(0);
        task->roots[1] = frame->response;
        if (frame->http_handler) {
            pxr_http_response(task, frame, frame->response, 0);
        } else if (frame->response->len > (uint64_t)frame->max_bytes) {
            pxr_task_fail(task, EMSGSIZE); return;
        }
        frame->phase = PXR_SERVER_WRITE;
    }
    for (unsigned budget = 0; budget < 64; budget++) {
        if (frame->phase == PXR_SERVER_READ) {
            char chunk[8192];
            size_t size = sizeof(chunk);
            if (frame->http_handler) {
                pxr_http_context *http = task->http;
                size_t remaining = !http->parsed ? 16384 - frame->request.len
                    : http->chunked ? sizeof(chunk)
                    : http->header_end + http->body_length - frame->request.len;
                if (size > remaining) size = remaining;
                assert(size); /* consume dispatches/errors before a bound is reached. */
            }
            ssize_t n = recv(frame->fd, chunk, size, 0);
            if (n < 0 && errno == EINTR) continue;
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                pxr_server_wait(task, frame, POLLIN); return;
            }
            if (n < 0) { pxr_task_fail(task, errno); return; }
            int ready = !n;
            if (frame->http_handler) {
                if (n) {
                    pxr_buf_append(&frame->request, chunk, (size_t)n);
                    task->roots[0] = frame->request.data;
                }
                ready = pxr_http_consume(task, frame, !n);
                if (ready >= 2) continue; /* Protocol error or interim response. */
            }
            if (ready) {
                if (frame->http_handler) {
                    frame->handler_task = frame->http_handler();
                } else {
                    pxr_str *request = pxr_str_alloc(frame->request.len);
                    if (frame->request.len) memcpy(request->data, frame->request.data, frame->request.len);
                    task->roots[1] = request; /* The constructor may collect. */
                    task->roots[0] = NULL;
                    pxr_heap_free(frame->request.data); frame->request.data = NULL;
                    frame->handler_task = frame->handler(request);
                    task->roots[1] = NULL; /* The lazy handler now retains its argument. */
                }
                if (frame->http_handler) {
                    task->roots[0] = NULL;
                    pxr_heap_free(frame->request.data); frame->request.data = NULL;
                }
                frame->phase = PXR_SERVER_HANDLER;
                pxr_task_await(task, frame->handler_task);
                if (task->state == PXR_TASK_CHILD) pxr_task_timer_at(task, frame->expires);
                return;
            }
            if (frame->http_handler) continue;
            if ((size_t)n > (uint64_t)frame->max_bytes - frame->request.len) {
                pxr_task_fail(task, EMSGSIZE); return;
            }
            pxr_buf_append(&frame->request, chunk, (size_t)n);
            task->roots[0] = frame->request.data;
        } else if (frame->phase == PXR_SERVER_SHUTDOWN) {
            if (shutdown(frame->fd, SHUT_WR) < 0) {
                if (errno == EINTR) { pxr_task_yield(task); return; }
                pxr_task_finish(task, 0, 0); return;
            }
            uint64_t now = task->loop->now(task->loop->clock_context);
            uint64_t deadline = now > UINT64_MAX - 100000000 ? UINT64_MAX : now + 100000000;
            if (deadline < frame->expires) frame->expires = deadline;
            frame->phase = PXR_SERVER_DRAIN; frame->sent = 0;
        } else if (frame->phase == PXR_SERVER_DRAIN) {
            /* Announce response EOF before bounded input draining, avoiding an
             * immediate reset when an error response rejects an unread body. */
            char discard[8192];
            ssize_t n = recv(frame->fd, discard, sizeof(discard), 0);
            if (n < 0 && errno == EINTR) continue;
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                pxr_server_wait(task, frame, POLLIN); return;
            }
            if (n > 0) frame->sent += (size_t)n;
            if (n <= 0 || frame->sent >= 65536) { pxr_task_finish(task, 0, 0); return; }
        } else {
            if (frame->sent == frame->response->len) {
                if (frame->phase == PXR_SERVER_CONTINUE) {
                    frame->response = NULL; task->roots[1] = NULL; frame->sent = 0;
                    frame->phase = PXR_SERVER_READ; continue;
                }
                if (frame->http_handler) { frame->phase = PXR_SERVER_SHUTDOWN; continue; }
                pxr_task_finish(task, 0, 0); return; /* Cleanup closes the send side. */
            }
            size_t size = frame->response->len - frame->sent;
            if (size > 8192) size = 8192;
            int flags = 0;
#ifdef MSG_NOSIGNAL
            flags = MSG_NOSIGNAL;
#endif
            ssize_t n = send(frame->fd, frame->response->data + frame->sent, size, flags);
            if (n < 0 && errno == EINTR) continue;
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                pxr_server_wait(task, frame, POLLOUT); return;
            }
            if (n <= 0) { pxr_task_fail(task, n < 0 ? errno : EPIPE); return; }
            frame->sent += (size_t)n;
        }
    }
    pxr_task_yield(task);
}
static int pxr_server_open(pxr_server_frame *frame) {
    if ((!frame->handler && !frame->http_handler) || frame->max_requests < 0 || frame->max_bytes < 0 ||
        frame->timeout_ms <= 0 || frame->max_concurrent < 1 || frame->max_concurrent > 1024)
        return EINVAL;
    struct sockaddr_storage address; socklen_t length;
    int error = pxr_tcp_address(frame->address, frame->port, &address, &length);
    if (error) return error;
    int family = ((struct sockaddr *)&address)->sa_family;
    frame->fd = socket(family, SOCK_STREAM, 0);
    if (frame->fd < 0) return errno;
    error = pxr_tcp_configure(frame->fd);
    if (error) return error;
    int yes = 1;
    if (setsockopt(frame->fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes)) < 0) return errno;
    if (family == AF_INET6 && setsockopt(frame->fd, IPPROTO_IPV6, IPV6_V6ONLY, &yes, sizeof(yes)) < 0) return errno;
    if (bind(frame->fd, (struct sockaddr *)&address, length) < 0) return errno;
    int backlog = frame->max_concurrent > SOMAXCONN ? SOMAXCONN : (int)frame->max_concurrent;
    if (listen(frame->fd, backlog) < 0) return errno;
    return 0;
}
static int pxr_server_accept_retry(int error) {
    if (error == EINTR || error == ECONNABORTED) return 1;
#ifdef __linux__
    /* Linux may return a pending connection's network error from accept. */
    if (error == ENETDOWN || error == EPROTO || error == ENOPROTOOPT ||
        error == EHOSTDOWN || error == ENONET || error == EHOSTUNREACH ||
        error == EOPNOTSUPP || error == ENETUNREACH) return 1;
#endif
    return 0;
}
static void pxr_server_resume(pxr_task *task, void *raw) {
    pxr_server_frame *frame = raw;
    if (!frame->started) {
        int error = pxr_server_open(frame);
        if (error) { pxr_task_fail(task, error); return; }
        frame->started = 1;
    }
    for (unsigned budget = 0; budget < 64; budget++) {
        if (frame->max_requests && !frame->remaining) {
            pxr_server_close(&frame->fd);
            pxr_task_finish(task, 0, 0); return; /* Join accepted connections. */
        }
        if (task->pending_children >= (uint64_t)frame->max_concurrent) {
            task->state = PXR_TASK_CHILDREN; return; /* Wake when a slot is released. */
        }
        int fd = accept(frame->fd, NULL, NULL);
        if (fd < 0) {
            int error = errno;
            if (pxr_server_accept_retry(error)) continue;
            if (error == EAGAIN || error == EWOULDBLOCK) {
                pxr_task_wait_fd(task, frame->fd, POLLIN); return;
            }
            pxr_task_fail(task, error); return;
        }
        /* Accepted sockets do not portably inherit listener flags. */
        int error = pxr_tcp_configure(fd);
        if (error) { pxr_server_close(&fd); pxr_task_fail(task, error); return; }
        uint64_t now = task->loop->now(task->loop->clock_context);
        pxr_task *child = pxr_task_new(task->loop, task, sizeof(pxr_server_connection),
            2, pxr_server_connection_resume, pxr_server_connection_cleanup);
        pxr_server_connection *connection = child->frame;
        connection->fd = fd; connection->max_bytes = frame->max_bytes;
        connection->handler = frame->handler;
        connection->http_handler = frame->http_handler;
        if (frame->http_handler) child->http = pxr_http_context_new(fd, frame->max_bytes);
        connection->expires = (uint64_t)frame->timeout_ms > (UINT64_MAX - now) / 1000000
            ? UINT64_MAX : now + (uint64_t)frame->timeout_ms * 1000000;
        pxr_task_roots(child, 2);
        pxr_task_start(child); pxr_task_release(child);
        if (frame->max_requests) frame->remaining--;
    }
    pxr_task_yield(task);
}
void *pxr_async_tcp_serve(pxr_str *address, int64_t port, pxr_tcp_handler handler,
                         int64_t max_requests, int64_t max_bytes,
                         int64_t timeout_ms, int64_t max_concurrent) {
    assert(pxr_compiler_loop_initialized && pxr_compiler_loop.current);
    pxr_task *task = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current,
        sizeof(pxr_server_frame), 1, pxr_server_resume, pxr_server_cleanup);
    pxr_server_frame *frame = task->frame;
    frame->fd = -1; frame->address = address; frame->port = port; frame->handler = handler;
    frame->remaining = frame->max_requests = max_requests; frame->max_bytes = max_bytes;
    frame->timeout_ms = timeout_ms; frame->max_concurrent = max_concurrent;
    task->roots[0] = address; pxr_task_roots(task, 1);
    return task;
}
