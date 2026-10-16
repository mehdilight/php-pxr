/* Native affine sockets. Resources belong to task scopes; operations hold
 * per-direction leases and never expose descriptors through source values. */
static _Thread_local size_t pxr_socket_resource_count;
static pxr_socket_resource *pxr_socket_get(pxr_object *object) {
    if (!object || object->remote_id != -2 || object->num_props != 1 || !object->bits[0]) return NULL;
    return (void *)(intptr_t)object->bits[0];
}
static void pxr_socket_link(pxr_socket_resource *resource, pxr_task *owner) {
    resource->owner = owner; resource->previous = NULL; resource->next = owner->resources;
    if (resource->next) resource->next->previous = resource;
    owner->resources = resource;
}
static void pxr_socket_unlink(pxr_socket_resource *resource) {
    if (resource->previous) resource->previous->next = resource->next;
    else resource->owner->resources = resource->next;
    if (resource->next) resource->next->previous = resource->previous;
}
static void pxr_socket_dispose(pxr_socket_resource *resource) {
    assert(!resource->reader && !resource->writer);
    pxr_socket_unlink(resource);
    resource->object->bits[0] = 0; resource->object->remote_id = -3;
    if (resource->dispose_payload) resource->dispose_payload(resource->payload, resource->kind);
    free(resource); pxr_socket_resource_count--;
}
static void pxr_socket_close_resource(pxr_socket_resource *resource) {
    if (resource->closed) return;
    resource->closed = 1;
    /* Fail pending operations before closing: wake removes their poll/timer
     * registrations, and their callbacks will never touch a reused fd. */
    if (resource->reader) pxr_task_fail(resource->reader, ECANCELED);
    if (resource->writer) pxr_task_fail(resource->writer, ECANCELED);
    pxr_server_close(&resource->fd);
    if (!resource->reader && !resource->writer) pxr_socket_dispose(resource);
}
static void pxr_socket_resources_cleanup(pxr_task *task) {
    while (task->resources) {
        assert(!task->resources->reader && !task->resources->writer);
        pxr_socket_close_resource(task->resources);
    }
}
static void pxr_socket_promote_result(pxr_task *task, int64_t tag, int64_t bits) {
    if (!task->parent || tag != PXR_TAG_OBJ || !bits) return;
    pxr_object *value = (void *)(intptr_t)bits;
    /* Outcome wrappers are the only native owning containers. */
    while (value->class_id == -100 && value->num_props == 4) {
        if (value->bits[0] != PXR_TASK_OK || value->tags[2] != PXR_TAG_OBJ || !value->bits[2]) return;
        value = (void *)(intptr_t)value->bits[2];
    }
    pxr_socket_resource *resource = pxr_socket_get(value);
    if (resource && resource->owner == task) {
        pxr_socket_unlink(resource); pxr_socket_link(resource, task->parent);
    }
}
static void pxr_socket_discard_result(int64_t tag, int64_t bits) {
    if (tag != PXR_TAG_OBJ || !bits) return;
    pxr_object *value = (void *)(intptr_t)bits;
    while (value->class_id == -100 && value->num_props == 4) {
        if (value->bits[0] != PXR_TASK_OK || value->tags[2] != PXR_TAG_OBJ || !value->bits[2]) return;
        value = (void *)(intptr_t)value->bits[2];
    }
    pxr_socket_resource *resource = pxr_socket_get(value);
    if (resource) pxr_socket_close_resource(resource);
}
static pxr_object *pxr_socket_new(pxr_task *owner, int fd, int listener, int64_t class_id) {
    pxr_object *object = pxr_object_new(class_id, NULL, 1);
    pxr_socket_resource *resource = calloc(1, sizeof(*resource)); if (!resource) pxr_oom();
    resource->object = object; resource->fd = fd; resource->listener = listener;
    object->remote_id = -2; object->tags[0] = PXR_TAG_INT; object->bits[0] = (intptr_t)resource;
    pxr_socket_link(resource, owner); pxr_socket_resource_count++;
    return object;
}
static void pxr_socket_error_resume(pxr_task *task, void *raw) { pxr_task_fail(task, *(int *)raw); }
static void pxr_socket_report_error(int error) {
    pxr_task *task = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current, sizeof(int), 0, pxr_socket_error_resume, NULL);
    *(int *)task->frame = error; pxr_task_start(task); pxr_task_release(task);
}
void pxr_socket_close(void *raw) {
    pxr_socket_resource *resource = pxr_socket_get(raw);
    if (!resource) { pxr_socket_report_error(EBADF); return; }
    pxr_socket_close_resource(resource);
}
void pxr_socket_shutdown_write(void *raw) {
    pxr_socket_resource *resource = pxr_socket_get(raw);
    if (!resource || resource->listener || resource->fd < 0) { pxr_socket_report_error(EBADF); return; }
    if (resource->writer) { pxr_socket_report_error(EBUSY); return; }
    if (shutdown(resource->fd, SHUT_WR) < 0) pxr_socket_report_error(errno);
}

enum { PXR_STREAM_LISTEN, PXR_STREAM_CONNECT, PXR_STREAM_ACCEPT, PXR_STREAM_READ, PXR_STREAM_WRITE };
typedef struct {
    int op, started, connecting, fd;
    pxr_object *object; pxr_socket_resource *resource;
    pxr_str *text; size_t offset;
    int64_t number, timeout_ms, class_id, backlog;
    uint64_t expires;
} pxr_stream_frame;
static void pxr_stream_cleanup(pxr_task *task, void *raw) {
    pxr_stream_frame *frame = raw; pxr_server_close(&frame->fd);
    pxr_socket_resource *resource = frame->resource;
    if (resource) {
        if (resource->reader == task) resource->reader = NULL;
        if (resource->writer == task) resource->writer = NULL;
        if (resource->closed && !resource->reader && !resource->writer) pxr_socket_dispose(resource);
    }
}
static void pxr_stream_wait(pxr_task *task, pxr_stream_frame *frame, short events) {
    pxr_task_wait_fd(task, frame->resource ? frame->resource->fd : frame->fd, events);
    if (task->state == PXR_TASK_IO) pxr_task_timer_at(task, frame->expires);
}
static pxr_task *pxr_stream_unused(pxr_str *text) { (void)text; abort(); }
static void pxr_stream_resume(pxr_task *task, void *raw) {
    pxr_stream_frame *frame = raw;
    uint64_t now = task->loop->now(task->loop->clock_context);
    if (!frame->started) {
        frame->started = 1;
        if (frame->timeout_ms < 0) { pxr_task_fail(task, EINVAL); return; }
        frame->expires = (uint64_t)frame->timeout_ms > (UINT64_MAX - now) / 1000000
            ? UINT64_MAX : now + (uint64_t)frame->timeout_ms * 1000000;
        if (frame->op >= PXR_STREAM_ACCEPT) {
            pxr_socket_resource *r = pxr_socket_get(frame->object);
            if (!r || r->closed || (frame->op == PXR_STREAM_ACCEPT) != r->listener) { pxr_task_fail(task, EBADF); return; }
            pxr_task **lease = frame->op == PXR_STREAM_WRITE ? &r->writer : &r->reader;
            if (*lease) { pxr_task_fail(task, EBUSY); return; }
            *lease = task; frame->resource = r;
        }
    }
    if (now >= frame->expires) { pxr_task_fail(task, ETIMEDOUT); return; }
    if (frame->op == PXR_STREAM_LISTEN) {
        pxr_server_frame config = {0}; config.fd = -1; config.handler = pxr_stream_unused;
        config.address = frame->text; config.port = frame->number; config.timeout_ms = 30000;
        config.max_concurrent = frame->backlog;
        int error = pxr_server_open(&config); frame->fd = config.fd;
        if (error) { pxr_task_fail(task, error); return; }
        pxr_object *object = pxr_socket_new(task, frame->fd, 1, frame->class_id);
        frame->fd = -1; pxr_task_finish(task, PXR_TAG_OBJ, (intptr_t)object); return;
    }
    if (frame->op == PXR_STREAM_CONNECT) {
        if (frame->fd < 0) {
            struct sockaddr_storage address; socklen_t length;
            int error = pxr_tcp_address(frame->text, frame->number, &address, &length);
            if (error) { pxr_task_fail(task, error); return; }
            frame->fd = socket(((struct sockaddr *)&address)->sa_family, SOCK_STREAM, 0);
            if (frame->fd < 0) { pxr_task_fail(task, errno); return; }
            error = pxr_tcp_configure(frame->fd); if (error) { pxr_task_fail(task, error); return; }
            int result = connect(frame->fd, (struct sockaddr *)&address, length);
            if (result < 0 && errno != EINPROGRESS && errno != EINTR) { pxr_task_fail(task, errno); return; }
            if (result < 0) { frame->connecting = 1; pxr_stream_wait(task, frame, POLLOUT); return; }
        }
        if (frame->connecting) {
            int error = 0; socklen_t length = sizeof(error);
            if (getsockopt(frame->fd, SOL_SOCKET, SO_ERROR, &error, &length) < 0) {
                if (errno == EINTR) pxr_task_yield(task); else pxr_task_fail(task, errno); return;
            }
            if (error == EINPROGRESS || error == EALREADY) { pxr_stream_wait(task, frame, POLLOUT); return; }
            if (error) { pxr_task_fail(task, error); return; }
            frame->connecting = 0;
        }
        pxr_object *object = pxr_socket_new(task, frame->fd, 0, frame->class_id);
        frame->fd = -1; pxr_task_finish(task, PXR_TAG_OBJ, (intptr_t)object); return;
    }
    pxr_socket_resource *r = frame->resource;
    if (frame->op == PXR_STREAM_ACCEPT) {
        int fd = accept(r->fd, NULL, NULL);
        if (fd < 0) {
            if (pxr_server_accept_retry(errno)) { pxr_task_yield(task); return; }
            if (errno == EAGAIN || errno == EWOULDBLOCK) pxr_stream_wait(task, frame, POLLIN);
            else pxr_task_fail(task, errno);
            return;
        }
        int error = pxr_tcp_configure(fd);
        if (error) { pxr_server_close(&fd); pxr_task_fail(task, error); return; }
        pxr_object *object = pxr_socket_new(task, fd, 0, frame->class_id);
        pxr_task_finish(task, PXR_TAG_OBJ, (intptr_t)object); return;
    }
    if (frame->op == PXR_STREAM_READ) {
        if (frame->number <= 0) { pxr_task_fail(task, EINVAL); return; }
        char bytes[8192]; size_t size = (uint64_t)frame->number < sizeof(bytes) ? (size_t)frame->number : sizeof(bytes);
        ssize_t n = recv(r->fd, bytes, size, 0);
        if (n < 0) {
            if (errno == EINTR) { pxr_task_yield(task); return; }
            if (errno == EAGAIN || errno == EWOULDBLOCK) pxr_stream_wait(task, frame, POLLIN);
            else pxr_task_fail(task, errno);
            return;
        }
        pxr_str *text = pxr_str_alloc((size_t)n); if (n) memcpy(text->data, bytes, (size_t)n);
        pxr_task_finish(task, PXR_TAG_STR, (intptr_t)text); return;
    }
    for (unsigned budget = 0; budget < 64; budget++) {
        if (frame->offset == frame->text->len) { pxr_task_finish(task, PXR_TAG_INT, (int64_t)frame->offset); return; }
        size_t nbytes = frame->text->len - frame->offset; if (nbytes > 8192) nbytes = 8192;
        int flags = 0;
#ifdef MSG_NOSIGNAL
        flags = MSG_NOSIGNAL;
#endif
        ssize_t n = send(r->fd, frame->text->data + frame->offset, nbytes, flags);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) { pxr_stream_wait(task, frame, POLLOUT); return; }
        if (n <= 0) { pxr_task_fail(task, n < 0 ? errno : EPIPE); return; }
        frame->offset += (size_t)n;
    }
    pxr_task_yield(task);
}
static pxr_task *pxr_stream_task(int op, pxr_object *object, pxr_str *text, int64_t number, int64_t timeout, int64_t class_id) {
    pxr_task *task = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current, sizeof(pxr_stream_frame), 2, pxr_stream_resume, pxr_stream_cleanup);
    pxr_stream_frame *frame = task->frame;
    frame->op = op; frame->object = object; frame->text = text; frame->number = number;
    frame->timeout_ms = timeout; frame->class_id = class_id; frame->fd = -1;
    task->roots[0] = object; task->roots[1] = text; pxr_task_roots(task, 2);
    return task;
}
void *pxr_async_stream_connect(pxr_str *address, int64_t port, int64_t timeout, int64_t class_id) {
    return pxr_stream_task(PXR_STREAM_CONNECT, NULL, address, port, timeout, class_id);
}
void *pxr_async_stream_listen(pxr_str *address, int64_t port, int64_t backlog, int64_t class_id) {
    /* Validate configuration only when the lazy task starts. */
    pxr_task *task = pxr_stream_task(PXR_STREAM_LISTEN, NULL, address, port, 30000, class_id);
    pxr_stream_frame *frame = task->frame;
    if (backlog < 1 || backlog > 1024) frame->timeout_ms = -1;
    else frame->backlog = backlog;
    return task;
}
void *pxr_async_stream_accept(void *listener, int64_t timeout, int64_t class_id) {
    return pxr_stream_task(PXR_STREAM_ACCEPT, listener, NULL, 0, timeout, class_id);
}
void *pxr_async_stream_read(void *stream, int64_t bytes, int64_t timeout) {
    return pxr_stream_task(PXR_STREAM_READ, stream, NULL, bytes, timeout, 0);
}
void *pxr_async_stream_write(void *stream, pxr_str *bytes, int64_t timeout) {
    return pxr_stream_task(PXR_STREAM_WRITE, stream, bytes, 0, timeout, 0);
}

void pxr_value_drop(int64_t tag, int64_t bits) { pxr_socket_discard_result(tag, bits); }

void pxr_async_adopt_resource(void *handle, void *value) {
    pxr_task *task = handle;
    if (!value) return;
    pxr_object *object = value;
    while (object->class_id == -100 && object->num_props == 4) {
        if (object->bits[0] != PXR_TASK_OK || object->tags[2] != PXR_TAG_OBJ || !object->bits[2]) return;
        object = (void *)(intptr_t)object->bits[2];
    }
    pxr_socket_resource *resource = pxr_socket_get(object);
    if (resource && resource->owner != task) {pxr_socket_unlink(resource); pxr_socket_link(resource, task);}
}
