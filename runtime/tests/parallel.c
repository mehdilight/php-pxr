/* Native assertions complement source acceptance: actual pthreads, independent
 * heaps, bounded queue storage, descriptors, cancellation and cleanup order. */
const char pxr_php_worker_source[] = "";
pxr_str *pxr_native_object_to_string(void *object) {(void)object; abort();}
static pthread_t parent_thread;
static atomic_int workers_entered, worker_gate;
static void bootstrap(void) {
    assert(!pthread_equal(parent_thread, pthread_self()));
    assert(pxr_worker_thread && !pxr_static_classes[0] && !pxr_supertypes[0]);
    pxr_class_register(256, 256);
    assert(pxr_supertypes[0] && pxr_heap_lookup(pxr_supertypes[0]));
    atomic_fetch_add(&workers_entered, 1);
}
typedef struct {int pc; int64_t value; pxr_task *child, *other; pxr_object *object;} frame;
static void work_resume(pxr_task *task, void *raw) {
    frame *f = raw;
    if (!f->pc++) {
        /* Every worker sees its own storage even though all use the same ID. */
        pxr_object *storage = pxr_static_storage(256, 1); assert(!storage->bits[0]);
        storage->tags[0] = PXR_TAG_INT; storage->bits[0] = f->value;
    }
    if (!atomic_load(&worker_gate)) {pxr_task_sleep(task, 1); return;}
    pxr_object *storage = pxr_static_storage(256, 1); assert(storage->bits[0] == f->value);
    pxr_task_finish(task, PXR_TAG_INT, f->value * 2);
}
static pxr_task *worker(int64_t tag, int64_t value) {
    assert(tag == PXR_TAG_INT);
    pxr_task *t = pxr_task_new(&pxr_compiler_loop, pxr_compiler_loop.current, sizeof(frame), 0, work_resume, NULL);
    ((frame *)t->frame)->value = value; return t;
}
static int cancel_mode;
static void parent_resume(pxr_task *task, void *raw) {
    frame *f = raw;
    if (!f->pc) {
        f->child = pxr_async_parallel(worker, PXR_TAG_INT, 21, bootstrap);
        pxr_task_start(f->child); f->pc = 1;
    }
    if (f->pc == 1) {
        if (!atomic_load(&workers_entered)) {pxr_task_sleep(task, 1); return;}
        assert(atomic_load(&pxr_parallel_active) == 1 && !pxr_supertypes[0]);
        if (cancel_mode) {pxr_task_cancel(f->child); pxr_task_release(f->child); pxr_task_finish(task, 0, 0); return;}
        atomic_store(&worker_gate, 1); f->pc = 2; pxr_task_await(task, f->child); return;
    }
    pxr_task_result v = pxr_task_take(f->child); assert(v.status == PXR_TASK_OK && v.tag == PXR_TAG_INT && v.bits == 42);
    pxr_task_finish(task, 0, 0);
}
static int resource_mode, peer_fd, original_fd;
static void socket_result(pxr_task *task, void *raw) {
    (void)raw;
    pxr_object *object = pxr_socket_new(task, original_fd, 0, 123);
    pxr_task_finish(task, PXR_TAG_OBJ, (intptr_t)object);
}
static void socket_parent(pxr_task *task, void *raw) {
    frame *f = raw;
    if (!f->pc++) {
        int fds[2]; assert(!socketpair(AF_UNIX, SOCK_STREAM, 0, fds));
        original_fd = fds[0]; peer_fd = fds[1]; assert(!pxr_tcp_configure(original_fd));
        if (resource_mode >= 2) {
            f->child = pxr_task_new(task->loop, task, 1, 0, socket_result, NULL);
            pxr_task_start(f->child); pxr_task_sleep(task, 0); return;
        }
        f->object = pxr_socket_new(task, original_fd, 0, 123);
        task->roots[0] = f->object; pxr_task_roots(task, 1);
        f->child = pxr_async_stream_read(f->object, 10, 30000);
        f->child->isolate_failure = 1; pxr_task_start(f->child);
        pxr_task_sleep(task, 0); return;
    }
    if (resource_mode == 4 && f->pc == 4) {
        errno = 0; assert(fcntl(original_fd, F_GETFD) == -1 && errno == EBADF);
        close(peer_fd); assert(!pxr_socket_resource_count); pxr_task_finish(task, 0, 0); return;
    }
    if (resource_mode < 2 && f->pc == 2) {
        assert(f->child->state == PXR_TASK_IO);
        if (resource_mode == 1) {
            f->other = pxr_async_stream_read(f->object, 1, 30000);
            f->other->isolate_failure = 1; pxr_task_await(task, f->other); return;
        }
        pxr_socket_close(f->object);
        errno = 0; assert(fcntl(original_fd, F_GETFD) == -1 && errno == EBADF);
        pxr_task_await(task, f->child); return;
    }
    if (resource_mode == 1 && f->pc == 3) {
        pxr_task_result busy = pxr_task_take(f->other); assert(busy.status == PXR_TASK_ERROR && busy.error == EBUSY);
        pxr_socket_close(f->object); pxr_task_await(task, f->child); return;
    }
    if (resource_mode < 2) {
        pxr_task_result v = pxr_task_take(f->child); assert(v.status == PXR_TASK_ERROR && v.error == ECANCELED);
    } else {
        assert(f->child->state == PXR_TASK_COMPLETE);
        if (resource_mode == 2) {pxr_task_release(f->child);} /* unclaimed resource result */
        else {
            if (f->pc == 2) {pxr_task_await(task, f->child); return;}
            pxr_task_result v = pxr_task_take(f->child);
            pxr_object *outcome = pxr_object_new(-100, NULL, 4);
            outcome->tags[0] = outcome->tags[1] = outcome->tags[3] = PXR_TAG_INT;
            outcome->bits[0] = PXR_TASK_OK; outcome->tags[2] = v.tag; outcome->bits[2] = v.bits;
            if (resource_mode == 4) {
                pxr_task *extraction = pxr_async_result_value_task(outcome);
                pxr_task_release(extraction); /* lazy extraction owns/closes the payload */
                pxr_task_sleep(task, 0); return;
            }
            pxr_value_drop(PXR_TAG_OBJ, (intptr_t)outcome);
        }
        errno = 0; assert(fcntl(original_fd, F_GETFD) == -1 && errno == EBADF);
    }
    close(peer_fd); assert(!pxr_socket_resource_count); pxr_task_finish(task, 0, 0);
}
static void end(void) {
    pxr_loop_destroy(&pxr_compiler_loop); pxr_compiler_loop_initialized = 0;
    assert(!pxr_socket_resource_count && !atomic_load(&pxr_parallel_active) && !atomic_load(&pxr_channel_active));
    pxr_gc_collect(); assert(!pxr_heap_bytes && !pxr_heap_blocks);
}
static void test_channels(void) {
    pxr_channel *c = calloc(1, sizeof(*c)); assert(c);
    assert(!pthread_mutex_init(&c->mutex, NULL));
    assert(!pxr_notify_pipe(c->notify[0]) && !pxr_notify_pipe(c->notify[1]));
    c->refs[0] = c->ever[0] = 1; c->capacity = 1; atomic_fetch_add(&pxr_channel_active, 1);
    for (int i = 0; i < 20; i++) {pxr_channel_retain(c, 1); pxr_channel_release(c, 3);}
    pxr_channel_release(c, 2); assert(!atomic_load(&pxr_channel_active));
    size_t budget = 1048576; int error = 0;
    pxr_str *text = pxr_str_alloc(1048577);
    pxr_message *m = pxr_message_copy(PXR_TAG_STR, (intptr_t)text, 0, 0, 0, &budget, &error);
    assert(!m && error == EMSGSIZE);
    budget = 1048576; error = 0;
    pxr_object *object = pxr_object_new(123, NULL, 0);
    m = pxr_message_copy(PXR_TAG_OBJ, (intptr_t)object, 0, 0, 0, &budget, &error);
    assert(!m && error == EINVAL);
    pxr_gc_collect(); assert(!pxr_heap_bytes);
}
int main(void) {
    alarm(20); parent_thread = pthread_self();
    for (int i = 0; i < 20; i++) {
        cancel_mode = i % 2; atomic_store(&workers_entered, 0); atomic_store(&worker_gate, 0);
        pxr_loop_init(&pxr_compiler_loop); pxr_compiler_loop_initialized = 1;
        pxr_task_new(&pxr_compiler_loop, NULL, sizeof(frame), 0, parent_resume, NULL);
        assert(pxr_loop_run(&pxr_compiler_loop).status == PXR_TASK_OK); end();
    }
    for (resource_mode = 0; resource_mode < 5; resource_mode++) {
        pxr_loop_init(&pxr_compiler_loop); pxr_compiler_loop_initialized = 1;
        pxr_task_new(&pxr_compiler_loop, NULL, sizeof(frame), 1, socket_parent, NULL);
        assert(pxr_loop_run(&pxr_compiler_loop).status == PXR_TASK_OK); end();
    }
    test_channels(); alarm(0); puts("parallel clean");
}
