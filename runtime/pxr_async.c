/* PXR single-thread async runtime, appended to pxr_rt.c by the driver.
 * Internal ABI for future compiler-generated resumable frames. No PHP syntax
 * uses this yet. See docs/ASYNC_RUNTIME.md for ownership and failure rules.
 * Original implementation; upstream reading is recorded in ASYNC_REFERENCES.md.
 */
#include <assert.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <time.h>

typedef struct pxr_task pxr_task;
typedef struct pxr_loop pxr_loop;
typedef void (*pxr_task_callback)(pxr_task *, void *);
typedef struct { int status, error; int64_t tag, bits; } pxr_task_result;
enum { PXR_TASK_OK, PXR_TASK_CANCELLED, PXR_TASK_ERROR };
enum { PXR_TASK_NEW, PXR_TASK_READY, PXR_TASK_RUNNING, PXR_TASK_TIMER,
       PXR_TASK_IO, PXR_TASK_CHILD, PXR_TASK_JOINING, PXR_TASK_COMPLETE };

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
    for (pxr_loop *loop = pxr_async_loops; loop; loop = loop->registered_next)
        for (pxr_task *task = loop->tasks; task; task = task->all_next) {
            pxr_gc_mark(task->frame);
            for (size_t i = 0; i < task->root_count; i++) pxr_gc_mark(task->roots[i]);
            pxr_gc_mark_value(task->result.tag, task->result.bits);
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
static void pxr_task_sleep(pxr_task *task, uint64_t milliseconds) {
    assert(task == task->loop->current && task->state == PXR_TASK_RUNNING);
    pxr_loop *loop = task->loop;
    uint64_t now = loop->now(loop->clock_context);
    task->deadline = milliseconds > (UINT64_MAX - now) / 1000000
        ? UINT64_MAX : now + milliseconds * 1000000;
    task->state = PXR_TASK_TIMER;
    if (loop->timer_count == loop->timer_capacity) {
        loop->timer_capacity = pxr_grow_capacity(loop->timer_capacity, pxr_size_add(loop->timer_count, 1));
        loop->timers = pxr_async_resize(loop->timers, loop->timer_capacity, sizeof(*loop->timers));
    }
    size_t i = loop->timer_count++;
    loop->timers[i] = task; task->timer_index = i;
    pxr_timer_up(loop, i);
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
