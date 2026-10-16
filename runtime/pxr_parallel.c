/* Original shared-nothing pthread workers and bounded duplex value channels.
 * Managed pointers never cross a thread boundary. See docs/PARALLEL.md. */
#include <pthread.h>
#include <stdatomic.h>

typedef struct pxr_channel pxr_channel;
typedef struct pxr_message pxr_message;
struct pxr_message {
    int64_t tag, bits, class_id;
    size_t length, count;
    char *bytes;
    pxr_message **items, **keys;
    pxr_channel *endpoint; int role;
    pxr_message *next;
};
struct pxr_channel {
    pthread_mutex_t mutex;
    pxr_message *head[2], *tail[2];
    size_t count[2], refs[2], capacity;
    int ever[2], notify[2][2];
};
static atomic_size_t pxr_parallel_active;
static atomic_size_t pxr_channel_active;
static void pxr_channel_release(void *raw, int kind);
static void pxr_message_free(pxr_message *message) {
    if (!message) return;
    for (size_t i = 0; i < message->count; i++) {
        pxr_message_free(message->items[i]);
        if (message->keys) pxr_message_free(message->keys[i]);
    }
    if (message->endpoint) pxr_channel_release(message->endpoint, message->role + 2);
    free(message->bytes); free(message->items); free(message->keys); free(message);
}
static int pxr_notify_pipe(int fds[2]) {
    if (pipe(fds) < 0) return errno;
    for (int i = 0; i < 2; i++) {
        int flags = fcntl(fds[i], F_GETFL);
        if (flags < 0 || fcntl(fds[i], F_SETFL, flags | O_NONBLOCK) < 0 ||
            fcntl(fds[i], F_SETFD, FD_CLOEXEC) < 0) {
            int error = errno; close(fds[0]); close(fds[1]); fds[0] = fds[1] = -1; return error;
        }
    }
    return 0;
}
static void pxr_notify(int fd) {
    char byte = 1; ssize_t n;
    do { n = write(fd, &byte, 1); } while (n < 0 && errno == EINTR);
    /* EAGAIN means the pipe already contains a wakeup. */
    assert(n == 1 || (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)));
}
static void pxr_channel_signal(pxr_channel *channel) {
    pxr_notify(channel->notify[0][1]); pxr_notify(channel->notify[1][1]);
}
static void pxr_channel_retain(pxr_channel *channel, int role) {
    pthread_mutex_lock(&channel->mutex);
    channel->refs[role]++; channel->ever[role] = 1;
    pthread_mutex_unlock(&channel->mutex);
}
static void pxr_channel_release(void *raw, int kind) {
    pxr_channel *channel = raw; int role = kind - 2;
    pthread_mutex_lock(&channel->mutex);
    assert(channel->refs[role]); channel->refs[role]--;
    pxr_channel_signal(channel);
    int empty = !channel->refs[0] && !channel->refs[1];
    pthread_mutex_unlock(&channel->mutex);
    if (empty) {
        for (int r = 0; r < 2; r++) {
            while (channel->head[r]) {
                pxr_message *next = channel->head[r]->next;
                pxr_message_free(channel->head[r]); channel->head[r] = next;
            }
            close(channel->notify[r][0]); close(channel->notify[r][1]);
        }
        pthread_mutex_destroy(&channel->mutex); free(channel);
        atomic_fetch_sub(&pxr_channel_active, 1);
    }
}
static pxr_object *pxr_channel_proxy(pxr_task *owner, pxr_channel *channel, int role, int64_t class_id) {
    /* Adopts an already retained role reference. */
    pxr_object *object = pxr_socket_new(owner, -1, 0, class_id);
    pxr_socket_resource *resource = pxr_socket_get(object);
    resource->kind = role + 2; resource->payload = channel; resource->dispose_payload = pxr_channel_release;
    return object;
}
static pxr_message *pxr_message_copy(int64_t tag, int64_t bits, int root_object,
                                    int endpoints, unsigned depth, size_t *budget, int *error) {
    if (depth > 64 || *budget < sizeof(pxr_message)) { *error = EMSGSIZE; return NULL; }
    *budget -= sizeof(pxr_message);
    pxr_message *m = calloc(1, sizeof(*m)); if (!m) pxr_oom();
    m->tag = tag; m->bits = bits;
    if (tag == PXR_TAG_MIXED) {
        const pxr_mixed *v = pxr_mixed_checked((void *)(intptr_t)bits);
        m->bits = 0; m->count = 1; m->items = calloc(1, sizeof(*m->items)); if (!m->items) pxr_oom();
        m->items[0] = pxr_message_copy(v->tag, v->bits, 0, 0, depth + 1, budget, error);
        if (*error) goto fail;
    } else if (tag == PXR_TAG_STR) {
        pxr_str *s = (void *)(intptr_t)bits;
        m->bits = 0; m->length = s ? s->len : 0;
        if (m->length > *budget) { *error = EMSGSIZE; goto fail; }
        *budget -= m->length; m->bytes = malloc(m->length ? m->length : 1); if (!m->bytes) pxr_oom();
        if (m->length) memcpy(m->bytes, s->data, m->length);
    } else if (tag == PXR_TAG_ARR || tag == PXR_TAG_OBJ) {
        m->bits = 0;
        if (tag == PXR_TAG_OBJ) {
            pxr_object *o = (void *)(intptr_t)bits;
            if (!root_object || !o) { *error = EINVAL; goto fail; }
            pxr_socket_resource *resource = pxr_socket_get(o);
            if (resource && resource->kind >= 2 && endpoints) {
                m->endpoint = resource->payload; m->role = resource->kind - 2; m->class_id = o->class_id;
                pxr_channel_retain(m->endpoint, m->role); return m;
            }
            if (o->remote_id || o->class_id < 0) { *error = EINVAL; goto fail; }
            m->class_id = o->class_id; m->count = o->num_props;
        } else {
            pxr_array *a = (void *)(intptr_t)bits; m->count = a ? a->len : 0;
        }
        if (m->count > *budget / (sizeof(pxr_message) + 2 * sizeof(void *))) { *error = EMSGSIZE; m->count = 0; goto fail; }
        m->items = calloc(m->count ? m->count : 1, sizeof(*m->items)); if (!m->items) pxr_oom();
        if (tag == PXR_TAG_ARR) { m->keys = calloc(m->count ? m->count : 1, sizeof(*m->keys)); if (!m->keys) pxr_oom(); }
        for (size_t i = 0; i < m->count; i++) {
            int64_t t, b;
            if (tag == PXR_TAG_ARR) {
                pxr_ent *e = &((pxr_array *)(intptr_t)bits)->ents[i]; t = e->vt; b = e->bits;
                m->keys[i] = pxr_message_copy(e->ks ? PXR_TAG_STR : PXR_TAG_INT,
                    e->ks ? (intptr_t)e->ks : e->ki, 0, 0, depth + 1, budget, error);
            } else { pxr_object *o = (void *)(intptr_t)bits; t = o->tags[i]; b = o->bits[i]; }
            if (*error) goto fail;
            m->items[i] = pxr_message_copy(t, b, 0, 0, depth + 1, budget, error);
            if (*error) goto fail;
        }
    } else if (tag < PXR_TAG_NULL || tag > PXR_TAG_BOOL) { *error = EINVAL; goto fail; }
    return m;
fail:
    pxr_message_free(m); return NULL;
}
static pxr_task_result pxr_message_read(pxr_message *m, pxr_task *owner) {
    pxr_task_result value = {PXR_TASK_OK, 0, m->tag, m->bits};
    if (m->tag == PXR_TAG_MIXED) {
        pxr_task_result v = pxr_message_read(m->items[0], owner);
        value.bits = (intptr_t)pxr_mixed_box(v.tag, v.bits);
    } else if (m->tag == PXR_TAG_STR) {
        pxr_str *s = pxr_str_alloc(m->length); if (m->length) memcpy(s->data, m->bytes, m->length);
        value.bits = (intptr_t)s;
    } else if (m->tag == PXR_TAG_ARR) {
        pxr_array *a = pxr_array_new(); value.bits = (intptr_t)a;
        for (size_t i = 0; i < m->count; i++) {
            pxr_task_result k = pxr_message_read(m->keys[i], owner), v = pxr_message_read(m->items[i], owner);
            pxr_array_set(a, k.tag, k.tag == PXR_TAG_INT ? k.bits : 0,
                k.tag == PXR_TAG_STR ? (void *)(intptr_t)k.bits : NULL, v.tag, v.bits);
        }
    } else if (m->tag == PXR_TAG_OBJ) {
        if (m->endpoint) {
            value.bits = (intptr_t)pxr_channel_proxy(owner, m->endpoint, m->role, m->class_id);
            m->endpoint = NULL; /* role adopted */
        } else {
            pxr_object *o = pxr_object_new(m->class_id, NULL, (int64_t)m->count); value.bits = (intptr_t)o;
            for (size_t i = 0; i < m->count; i++) {
                pxr_task_result v = pxr_message_read(m->items[i], owner); o->tags[i] = v.tag; o->bits[i] = v.bits;
            }
        }
    }
    return value;
}

enum { PXR_CHANNEL_NEW, PXR_CHANNEL_PEER, PXR_CHANNEL_CLONE, PXR_CHANNEL_SEND, PXR_CHANNEL_RECV };
typedef struct {
    int op, started; int64_t capacity, class_id, timeout;
    pxr_object *object; pxr_mixed *value;
    pxr_socket_resource *resource; pxr_message *message;
    uint64_t expires;
} pxr_channel_frame;
static void pxr_channel_cleanup(pxr_task *task, void *raw) {
    pxr_channel_frame *f = raw; pxr_message_free(f->message);
    pxr_socket_resource *r = f->resource;
    if (r) {
        if (r->reader == task) r->reader = NULL;
        if (r->writer == task) r->writer = NULL;
        if (r->closed && !r->reader && !r->writer) pxr_socket_dispose(r);
    }
}
static void pxr_channel_resume(pxr_task *task, void *raw) {
    pxr_channel_frame *f = raw;
    uint64_t now = task->loop->now(task->loop->clock_context);
    if (!f->started) {
        f->started = 1;
        if (f->timeout < 0) { pxr_task_fail(task, EINVAL); return; }
        f->expires = (uint64_t)f->timeout > (UINT64_MAX - now) / 1000000 ? UINT64_MAX : now + (uint64_t)f->timeout * 1000000;
        if (f->op != PXR_CHANNEL_NEW) {
            pxr_socket_resource *r = pxr_socket_get(f->object);
            if (!r || r->closed || r->kind < 2) { pxr_task_fail(task, EBADF); return; }
            f->resource = r;
            pxr_task **lease = f->op == PXR_CHANNEL_SEND ? &r->writer : &r->reader;
            if (*lease) { f->resource = NULL; pxr_task_fail(task, EBUSY); return; }
            *lease = task;
        }
        if (f->op == PXR_CHANNEL_SEND) {
            size_t budget = 1048576; int error = 0;
            f->message = pxr_message_copy(f->value->tag, f->value->bits, 0, 0, 0, &budget, &error);
            if (error) { pxr_task_fail(task, error); return; }
        }
    }
    if (f->op == PXR_CHANNEL_NEW) {
        if (f->capacity < 1 || f->capacity > 1024) { pxr_task_fail(task, EINVAL); return; }
        pxr_channel *c = calloc(1, sizeof(*c)); if (!c) pxr_oom();
        int error = pthread_mutex_init(&c->mutex, NULL);
        if (error) { free(c); pxr_task_fail(task, error); return; }
        error = pxr_notify_pipe(c->notify[0]);
        if (!error) {
            error = pxr_notify_pipe(c->notify[1]);
            if (error) { close(c->notify[0][0]); close(c->notify[0][1]); }
        }
        if (error) { pthread_mutex_destroy(&c->mutex); free(c); pxr_task_fail(task, error); return; }
        c->refs[0] = c->ever[0] = 1; c->capacity = (size_t)f->capacity;
        atomic_fetch_add(&pxr_channel_active, 1);
        pxr_object *o = pxr_channel_proxy(task, c, 0, f->class_id);
        pxr_task_finish(task, PXR_TAG_OBJ, (intptr_t)o); return;
    }
    pxr_socket_resource *r = f->resource; pxr_channel *c = r->payload; int role = r->kind - 2;
    if (f->op == PXR_CHANNEL_PEER || f->op == PXR_CHANNEL_CLONE) {
        int target = f->op == PXR_CHANNEL_PEER ? 1 - role : role;
        pthread_mutex_lock(&c->mutex);
        if (c->ever[target] && !c->refs[target]) {
            pthread_mutex_unlock(&c->mutex); pxr_task_fail(task, EPIPE); return;
        }
        c->refs[target]++; c->ever[target] = 1;
        pthread_mutex_unlock(&c->mutex);
        pxr_object *o = pxr_channel_proxy(task, c, target, f->object->class_id);
        pxr_task_finish(task, PXR_TAG_OBJ, (intptr_t)o); return;
    }
    pthread_mutex_lock(&c->mutex);
    /* Drain hints while locked: no producer can publish between the condition
     * check and draining, so a waiter never loses its only wakeup. */
    char bytes[128]; while (read(c->notify[role][0], bytes, sizeof(bytes)) > 0) {}
    int peer_closed = c->ever[1 - role] && !c->refs[1 - role];
    if (f->op == PXR_CHANNEL_RECV && c->head[role]) {
        pxr_message *m = c->head[role]; c->head[role] = m->next; c->count[role]--;
        if (!c->head[role]) c->tail[role] = NULL;
        pxr_channel_signal(c); pthread_mutex_unlock(&c->mutex);
        pxr_task_result v = pxr_message_read(m, task); pxr_message_free(m);
        void *boxed = v.tag == PXR_TAG_MIXED ? (void *)(intptr_t)v.bits : pxr_mixed_box(v.tag, v.bits);
        pxr_task_finish(task, PXR_TAG_MIXED, (intptr_t)boxed); return;
    }
    if (peer_closed) { pthread_mutex_unlock(&c->mutex); pxr_task_fail(task, EPIPE); return; }
    if (f->op == PXR_CHANNEL_SEND && c->count[1 - role] < c->capacity) {
        int target = 1 - role; pxr_message *m = f->message; f->message = NULL;
        if (c->tail[target]) c->tail[target]->next = m; else c->head[target] = m;
        c->tail[target] = m; c->count[target]++;
        pxr_channel_signal(c); pthread_mutex_unlock(&c->mutex);
        pxr_task_finish(task, 0, 0); return;
    }
    pthread_mutex_unlock(&c->mutex);
    if (now >= f->expires) { pxr_task_fail(task, ETIMEDOUT); return; }
    pxr_task_wait_fd(task, c->notify[role][0], POLLIN);
    if (task->state == PXR_TASK_IO) pxr_task_timer_at(task, f->expires);
}
void *pxr_async_channel(int64_t op, void *object, void *value, int64_t number, int64_t class_id) {
    pxr_task *task = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current,
        sizeof(pxr_channel_frame), 2, pxr_channel_resume, pxr_channel_cleanup);
    pxr_channel_frame *f = task->frame; f->op = (int)op; f->object = object; f->value = value;
    f->capacity = number; f->timeout = op == PXR_CHANNEL_NEW ? 30000 : number; f->class_id = class_id;
    task->roots[0] = object; task->roots[1] = value; pxr_task_roots(task, 2); return task;
}

typedef pxr_task *(*pxr_parallel_callback)(int64_t, int64_t);
typedef struct {
    pthread_t thread;
    int started, joined, done_pipe[2], cancel_pipe[2];
    atomic_int done, cancel;
    pxr_parallel_callback callback; void (*bootstrap)(void);
    pxr_message *input, *output;
    int status, error;
} pxr_parallel_job;
typedef struct { int pc; pxr_parallel_job *job; pxr_task *child; } pxr_worker_frame;
static int pxr_worker_cancelled(void *raw) { return atomic_load(&((pxr_parallel_job *)raw)->cancel); }
static void pxr_worker_resume(pxr_task *task, void *raw) {
    pxr_worker_frame *f = raw; pxr_parallel_job *j = f->job;
    if (!f->pc++) {
        j->bootstrap();
        pxr_task_result v = pxr_message_read(j->input, task); pxr_message_free(j->input); j->input = NULL;
        task->roots[0] = v.tag >= PXR_TAG_STR ? (void *)(intptr_t)v.bits : NULL; pxr_task_roots(task, 1);
        f->child = j->callback(v.tag, v.bits); pxr_task_await(task, f->child); return;
    }
    pxr_task_result v = pxr_task_take(f->child);
    size_t budget = 1048576; int error = 0;
    j->output = pxr_message_copy(v.tag, v.bits, 1, 1, 0, &budget, &error);
    if (error) { pxr_task_fail(task, error); return; }
    pxr_task_finish(task, 0, 0);
}
static void *pxr_parallel_worker(void *raw) {
    pxr_parallel_job *j = raw; pxr_worker_thread = 1;
    pxr_loop_init(&pxr_compiler_loop); pxr_compiler_loop_initialized = 1;
    pxr_compiler_loop.cancel_fd = j->cancel_pipe[0];
    pxr_compiler_loop.cancelled = pxr_worker_cancelled; pxr_compiler_loop.cancel_context = j;
    pxr_task *root = pxr_task_new(&pxr_compiler_loop, NULL, sizeof(pxr_worker_frame), 1, pxr_worker_resume, NULL);
    ((pxr_worker_frame *)root->frame)->job = j;
    pxr_task_result v = pxr_loop_run(&pxr_compiler_loop); j->status = v.status; j->error = v.error;
    pxr_loop_destroy(&pxr_compiler_loop);
    assert(!pxr_socket_resource_count && !pxr_gc_frames && !pxr_async_loops);
    pxr_heap_shutdown();
    /* Publication includes all cleanup, not just the user callback result. */
    atomic_store(&j->done, 1); pxr_notify(j->done_pipe[1]); return NULL;
}
static void pxr_parallel_cleanup(pxr_task *task, void *raw) {
    (void)task; pxr_parallel_job *j = ((pxr_worker_frame *)raw)->job;
    assert(!j->started || j->joined);
    pxr_message_free(j->input); pxr_message_free(j->output);
    for (int i = 0; i < 2; i++) { pxr_server_close(&j->done_pipe[i]); pxr_server_close(&j->cancel_pipe[i]); }
    free(j);
}
static void pxr_parallel_resume(pxr_task *task, void *raw) {
    pxr_parallel_job *j = ((pxr_worker_frame *)raw)->job;
    if (!j->started) {
        if (task->cancel_requested) { task->defer_cancel = 0; pxr_task_finish(task, 0, 0); return; }
        if (j->error) { pxr_task_fail(task, j->error); return; }
        size_t active = atomic_fetch_add(&pxr_parallel_active, 1);
        if (active >= 64) { atomic_fetch_sub(&pxr_parallel_active, 1); pxr_task_fail(task, EAGAIN); return; }
        int error = pxr_notify_pipe(j->done_pipe);
        if (!error) error = pxr_notify_pipe(j->cancel_pipe);
        if (!error) error = pthread_create(&j->thread, NULL, pxr_parallel_worker, j);
        if (error) { atomic_fetch_sub(&pxr_parallel_active, 1); pxr_task_fail(task, error); return; }
        j->started = 1; task->defer_cancel = 1;
    }
    if (task->cancel_requested) { atomic_store(&j->cancel, 1); pxr_notify(j->cancel_pipe[1]); }
    if (!atomic_load(&j->done)) { pxr_task_wait_fd(task, j->done_pipe[0], POLLIN); return; }
    int join_error = pthread_join(j->thread, NULL); if (join_error) abort();
    j->joined = 1; task->defer_cancel = 0;
    atomic_fetch_sub(&pxr_parallel_active, 1);
    if (task->cancel_requested) { pxr_task_finish(task, 0, 0); return; }
    if (j->status != PXR_TASK_OK) { pxr_task_fail(task, j->error ? j->error : ECANCELED); return; }
    pxr_task_result v = pxr_message_read(j->output, task); pxr_message_free(j->output); j->output = NULL;
    pxr_task_finish(task, v.tag, v.bits);
}
void *pxr_async_parallel(pxr_parallel_callback callback, int64_t tag, int64_t bits, void (*bootstrap)(void)) {
    pxr_task *task = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current,
        sizeof(pxr_worker_frame), 0, pxr_parallel_resume, pxr_parallel_cleanup);
    pxr_parallel_job *j = calloc(1, sizeof(*j)); if (!j) pxr_oom();
    j->done_pipe[0] = j->done_pipe[1] = j->cancel_pipe[0] = j->cancel_pipe[1] = -1;
    atomic_init(&j->done, 0); atomic_init(&j->cancel, 0);
    j->callback = callback; j->bootstrap = bootstrap;
    size_t budget = 1048576;
    j->input = pxr_message_copy(tag, bits, 1, 1, 0, &budget, &j->error);
    /* A channel token now owns the transferred reference; release the source
     * proxy immediately so worker exit can close the peer promptly. */
    pxr_socket_discard_result(tag, bits);
    ((pxr_worker_frame *)task->frame)->job = j; return task;
}
