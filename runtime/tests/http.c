/* Parser, request-root and teardown tests against the embedded HTTP adapter. */
const char pxr_php_worker_source[] = "";
pxr_str *pxr_native_object_to_string(void *object) { (void)object; abort(); }
static const char *input;
static size_t input_len, input_at, feed;
static int expected_status, inspect, header_error;
static const char *expected_body;
static int64_t parser_limit = 4096;
static pxr_task *unused_handler(void) { abort(); }
static const pxr_mixed *field(pxr_array *array, const char *key) {
    pxr_str *name = pxr_http_string(key, strlen(key));
    return pxr_mixed_checked((void *)(intptr_t)pxr_array_get_bits(array, PXR_TAG_STR, 0, name));
}
static pxr_str *text_field(pxr_array *array, const char *key) {
    const pxr_mixed *value = field(array, key); assert(value->tag == PXR_TAG_STR);
    return (void *)(intptr_t)value->bits;
}
static void text_equals(pxr_str *text, const char *expected) {
    assert(text->len == strlen(expected) && !memcmp(text->data, expected, text->len));
}
static void inspect_request(pxr_http_context *context) {
    text_equals(text_field(context->globals[0], "a_b"), "last");
    const pxr_mixed *tags = field(context->globals[0], "tags"); assert(tags->tag == PXR_TAG_ARR);
    pxr_array *array = (void *)(intptr_t)tags->bits; assert(array->len == 2);
    const pxr_mixed *first = pxr_mixed_checked((void *)(intptr_t)pxr_array_get_bits(array, PXR_TAG_INT, 0, NULL));
    text_equals((void *)(intptr_t)first->bits, "one");
    text_equals(text_field(context->globals[2], "id"), "a+b");
    text_equals(text_field(context->globals[3], "REQUEST_METHOD"), "POST");
    text_equals(text_field(context->globals[1], "x"), "form");
    text_equals(text_field(context->globals[4], "x"), "form");
    const pxr_mixed *merged = field(context->globals[4], "user"); assert(merged->tag == PXR_TAG_ARR);
    text_equals(text_field((void *)(intptr_t)merged->bits, "first"), "get");
    text_equals(text_field((void *)(intptr_t)merged->bits, "last"), "post");
    assert(!pxr_array_has(context->globals[2], PXR_TAG_STR, 0, pxr_http_string("__Host-forged", 13)));
    /* REQUEST and copied GET must not share mutable nested array identities. */
    pxr_http_literal(context->globals[0], "x", "changed");
    text_equals(text_field(context->globals[4], "x"), "form");
    pxr_print_str(pxr_http_string("live:", 5)); pxr_print_i64(42); pxr_print_bool(1); pxr_print_f64(1.5);
    pxr_gc_collect();
    assert(context->output.len == 11 && !memcmp(context->output.data, "live:4211.5", 11));
}
static void parse_resume(pxr_task *task, void *raw) {
    pxr_server_connection *frame = raw;
    if (!task->http) {
        task->http = pxr_http_context_new(-1, parser_limit);
        frame->fd = -1; frame->http_handler = unused_handler; pxr_task_roots(task, 2);
    }
    size_t remaining = input_len - input_at;
    size_t n = remaining < feed ? remaining : feed;
    if (!task->http->parsed && n > 16384 - frame->request.len) n = 16384 - frame->request.len;
    if (n) { pxr_buf_append(&frame->request, input + input_at, n); input_at += n; task->roots[0] = frame->request.data; }
    int ready = pxr_http_consume(task, frame, input_at == input_len);
    pxr_gc_collect();
    if (!ready || ready == 3) { frame->phase = PXR_SERVER_READ; pxr_task_yield(task); return; }
    if (ready == 1) {
        if (expected_body) text_equals(task->http->body, expected_body);
        if (inspect) inspect_request(task->http);
        if (header_error) pxr_http_header(pxr_http_string("Content-Length: 3", 17), 1, 0);
        else pxr_http_header(pxr_http_string("X-Case: yes", 11), 1, 0);
        pxr_http_response(task, frame, pxr_http_string("ok", 2), 0);
    }
    char prefix[32]; int length = snprintf(prefix, sizeof(prefix), "HTTP/1.1 %d ", expected_status);
    assert(frame->response->len > (size_t)length && !memcmp(frame->response->data, prefix, (size_t)length));
    pxr_gc_collect();
    assert(strstr(frame->response->data, "Connection: close\r\n"));
    if (inspect) assert(strstr(frame->response->data, "\r\n\r\nlive:4211.5ok"));
    pxr_task_finish(task, 0, 0);
}
static void run_parser(const char *request, size_t length, int status, size_t chunk) {
    input = request; input_len = length; input_at = 0; feed = chunk; expected_status = status;
    pxr_task *root = pxr_async_create(parse_resume, (sizeof(pxr_server_connection) + 7) / 8);
    root->cleanup = pxr_server_connection_cleanup;
    assert(pxr_loop_run(&pxr_compiler_loop).status == PXR_TASK_OK);
    pxr_loop_destroy(&pxr_compiler_loop); pxr_compiler_loop_initialized = 0;
    pxr_gc_collect();
    assert(!pxr_heap_blocks && !pxr_heap_bytes && !pxr_async_loops);
}
static void parser_cases(void) {
    const char *valid = "POST /?a.b=last&tags[%20]=one&tags[]=two&x=get&user[first]=get HTTP/1.1\r\nHost: localhost\r\nCookie: id=a+b; id=second; ..Host-forged=bad\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 22\r\n\r\nx=form&user[last]=post";
    /* Use the exact body length so a framing mistake cannot mask parameter tests. */
    assert(strlen("x=form&user[last]=post") == 22);
    inspect = 1; run_parser(valid, strlen(valid), 200, 1); inspect = 0;
    const char *requests[] = {
        "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "GET / HTTP/1.0\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: [::1]:8080\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: localhost\r\nX-Nul: a\0b\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: localhost\r\nX: a\nb\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: user@localhost\r\n\r\n",
        "GET /bad%xy HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2, 2\r\n\r\n",
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\nx",
        "GET / HTTP/1.1\r\nHost: localhost\r\nContent-Encoding: gzip\r\n\r\n",
    };
    int statuses[] = {200, 200, 200, 400, 400, 400, 400, 400, 400, 415};
    for (size_t i = 0; i < sizeof(requests) / sizeof(*requests); i++)
        run_parser(requests[i], i == 3 ? sizeof("GET / HTTP/1.1\r\nHost: localhost\r\nX-Nul: a\0b\r\n\r\n") - 1 : strlen(requests[i]), statuses[i], 7);
    header_error = 1; run_parser(requests[0], strlen(requests[0]), 500, 8192); header_error = 0;
    char *large = malloc(17000); assert(large); memset(large, 'a', 17000);
    memcpy(large, "GET / HTTP/1.1\r\nHost: localhost\r\nX: ", 35);
    run_parser(large, 17000, 431, 8192); free(large);
    /* Deterministic arbitrary-byte and truncation coverage under ASan/UBSan. */
    uint32_t random = 71823;
    for (unsigned i = 0; i < 128; i++) {
        char bytes[513]; size_t n = i * 4 + 1;
        for (size_t j = 0; j < n; j++) { random = random * 1664525u + 1013904223u; bytes[j] = (char)(random >> 24); }
        run_parser(bytes, n, 400, (i % 17) + 1);
    }
}
static void chunk_cases(void) {
    const char *header = "POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n";
    struct { const char *wire; int status; const char *body; } cases[] = {
        {"0\r\n\r\n", 200, ""},
        {"000\r\n\r\n", 200, ""},
        {"1\r\na\r\n2\r\nbc\r\n0\r\n\r\n", 200, "abc"},
        {"3 ; foo = \"a\\\"b\" ; bar=token\r\nabc\r\n0;end\r\nX-Checksum: ignored\r\n\r\n", 200, "abc"},
        {"1;x=\"\"\r\na\r\n0\r\n\r\n", 200, "a"},
        {"0\r\nContent-Length: 0\r\n\r\n", 400, NULL},
        {"0\r\nCookie: evil=1\r\n\r\n", 400, NULL},
        {"0\r\nHost: forged\r\n\r\n", 400, NULL},
        {"0\r\n X: folded\r\n\r\n", 400, NULL},
        {"0\r\nX: bad\nvalue\r\n\r\n", 400, NULL},
        {"1\r\naX\n0\r\n\r\n", 400, NULL},
        {"1\r\na\rX0\r\n\r\n", 400, NULL},
        {"1\r\na\r\n", 400, NULL},
        {"0\r\n", 400, NULL},
        {"1\r\n", 400, NULL},
        {"g\r\n", 400, NULL},
        {"+1\r\n", 400, NULL},
        {" 1\r\n", 400, NULL},
        {"1 \r\na\r\n0\r\n\r\n", 400, NULL},
        {"1;\r\na\r\n0\r\n\r\n", 400, NULL},
        {"1;x=\r\na\r\n0\r\n\r\n", 400, NULL},
        {"1;x=\"unterminated\r\na\r\n0\r\n\r\n", 400, NULL},
        {"1;x=\"bad\001\"\r\na\r\n0\r\n\r\n", 400, NULL},
        {"1001\r\n", 413, NULL},
        {"ffffffffffffffffffffffffffffffff\r\n", 413, NULL},
    };
    size_t feeds[] = {1, 2, 7, 8192};
    for (size_t i = 0; i < sizeof(cases) / sizeof(*cases); i++) {
        size_t n = strlen(header) + strlen(cases[i].wire);
        char *request = malloc(n + 1); assert(request);
        strcpy(request, header); strcat(request, cases[i].wire);
        expected_body = cases[i].body;
        for (size_t j = 0; j < sizeof(feeds) / sizeof(*feeds); j++)
            run_parser(request, n, cases[i].status, feeds[j]);
        free(request);
    }
    expected_body = NULL;
    /* Every truncation of a complete body must fail before dispatch. */
    const char *body = "3;x=y\r\nabc\r\n0\r\nX-Checksum: ignored\r\n\r\n";
    char request[512]; strcpy(request, header); strcat(request, body);
    for (size_t i = strlen(header); i < strlen(request); i++)
        run_parser(request, i, 400, 1);
    /* Metadata has separate limits from decoded content. */
    char *large = malloc(90000); assert(large); strcpy(large, header);
    size_t at = strlen(header);
    memset(large + at, 'a', 8193); at += 8193;
    run_parser(large, at, 413, 1);
    strcpy(large, header); at = strlen(header);
    for (int i = 0; i < 101; i++) {
        const char *line = i ? "X: ok\r\n" : "0\r\nX: ok\r\n";
        memcpy(large + at, line, strlen(line)); at += strlen(line);
    }
    memcpy(large + at, "\r\n", 2); at += 2;
    run_parser(large, at, 431, 7);
    strcpy(large, header); at = strlen(header);
    for (int i = 0; i < 9; i++) {
        memcpy(large + at, "1;x=", 4); at += 4;
        memset(large + at, 'a', 8000); at += 8000;
        memcpy(large + at, "\r\na\r\n", 5); at += 5;
    }
    memcpy(large + at, "0\r\n\r\n", 5); at += 5;
    run_parser(large, at, 413, 8192);
    strcpy(large, header); at = strlen(header);
    memcpy(large + at, "0\r\n", 3); at += 3;
    for (int i = 0; i < 6; i++) {
        memcpy(large + at, "X: ", 3); at += 3;
        memset(large + at, 'a', 3000); at += 3000;
        memcpy(large + at, "\r\n", 2); at += 2;
    }
    run_parser(large, at, 431, 7);
    strcpy(large, header); at = strlen(header);
    for (int i = 0; i < 8193; i++) {
        memcpy(large + at, "1\r\na\r\n", 6); at += 6;
    }
    parser_limit = 9000; run_parser(large, at, 413, 8192); parser_limit = 4096;
    free(large);
}
static int pair[2], mode, cleaned, closed_fd, waits;
static uint64_t now;
static uint64_t fake_now(void *context) { (void)context; return now; }
static int fake_wait(struct pollfd *fds, nfds_t count, int timeout, void *context) {
    (void)context;
    if (mode == 6) { assert(count == 1 && timeout == 0); return 0; }
    assert(count == 1 && timeout == 5);
    assert(fds[0].events == (mode == 5 ? POLLOUT : POLLIN));
    waits++; now += 5000000; return 0;
}
static void connection_cleanup(pxr_task *task, void *raw) {
    cleaned++; closed_fd = ((pxr_server_connection *)raw)->fd;
    pxr_gc_collect(); pxr_server_connection_cleanup(task, raw);
}
typedef struct { int pc; pxr_task *connection; } parent_frame;
static void sleeping_child(pxr_task *task, void *raw) {
    int *pc = raw;
    if (!(*pc)++) pxr_task_sleep(task, 1000000);
    else pxr_task_finish(task, 0, 0);
}
static void captured_handler(pxr_task *task, void *raw) {
    (void)raw;
    pxr_print_str(pxr_http_string("captured", 8));
    pxr_task *child = pxr_task_new(task->loop, task, sizeof(int), 0, sleeping_child, NULL);
    pxr_task_start(child); pxr_task_release(child);
    pxr_task_finish(task, 0, 0); /* Still joins its descendant. */
}
static pxr_task *new_captured_handler(void) { return pxr_async_create(captured_handler, 1); }
static void io_parent(pxr_task *task, void *raw) {
    parent_frame *parent = raw;
    if (!parent->pc++) {
        pxr_task *child = pxr_task_new(task->loop, task, sizeof(pxr_server_connection), 2,
            pxr_server_connection_resume, connection_cleanup);
        parent->connection = child;
        pxr_server_connection *frame = child->frame;
        frame->fd = pair[0]; frame->max_bytes = 4096; frame->expires = 5000000;
        frame->http_handler = unused_handler;
        child->http = pxr_http_context_new(-1, 4096); pxr_task_roots(child, 2);
        frame->phase = mode ? PXR_SERVER_READ : PXR_SERVER_DRAIN;
        if (mode == 2 || mode == 6) {
            if (mode == 2) frame->http_handler = new_captured_handler;
            pxr_task_start(child); pxr_task_sleep(task, 0); return;
        }
        pxr_task_await(task, child); return;
    }
    if (mode == 6) {
        pxr_task *connection = parent->connection;
        assert(connection->state == PXR_TASK_IO);
        pxr_gc_collect();
        assert(connection->http->decoded.len == 3 && !memcmp(connection->http->decoded.data, "abc", 3));
        pxr_async_cancel(connection); pxr_task_finish(task, 0, 0); return;
    }
    if (mode == 2) {
        pxr_task *connection = parent->connection;
        assert(connection->state == PXR_TASK_CHILD && connection->pending_children == 1);
        assert(connection->children->state == PXR_TASK_JOINING && connection->children->pending_children == 1);
        pxr_gc_collect();
        assert(connection->http->output.len == 8 && !memcmp(connection->http->output.data, "captured", 8));
        pxr_async_cancel(connection); pxr_task_finish(task, 0, 0); return;
    }
    assert(pxr_task_take(parent->connection).status == PXR_TASK_OK);
    pxr_task_finish(task, 0, 0);
}
static void teardown_cases(void) {
    for (mode = 0; mode < 7; mode++) {
        assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
        assert(pxr_tcp_configure(pair[0]) == 0);
        if (mode == 1) {
            const char *partial = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\n\r\nabc";
            assert(send(pair[1], partial, strlen(partial), 0) == (ssize_t)strlen(partial));
        } else if (mode == 2) {
            const char *request = "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
            assert(send(pair[1], request, strlen(request), 0) == (ssize_t)strlen(request));
        } else if (mode == 3 || mode == 6) {
            const char *partial = "POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n8\r\nabc";
            assert(send(pair[1], partial, strlen(partial), 0) == (ssize_t)strlen(partial));
        } else if (mode == 4 || mode == 5) {
            if (mode == 5) {
                /* Force the interim write to suspend on a full socket buffer. */
                char bytes[8192]; memset(bytes, 'x', sizeof(bytes));
                while (send(pair[0], bytes, sizeof(bytes), 0) > 0) {}
                assert(errno == EAGAIN || errno == EWOULDBLOCK);
            }
            const char *partial = "POST / HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 10\r\n\r\n";
            assert(send(pair[1], partial, strlen(partial), 0) == (ssize_t)strlen(partial));
        }
        pxr_async_create(io_parent, (sizeof(parent_frame) + 7) / 8);
        now = 0; waits = cleaned = 0; closed_fd = -1;
        pxr_compiler_loop.now = fake_now; pxr_compiler_loop.wait = fake_wait;
        pxr_task_result result = pxr_loop_run(&pxr_compiler_loop);
        assert(cleaned == 1);
        if (mode == 2 || mode == 6) assert(!waits && !now);
        else assert(waits == 1 && now == 5000000);
        int failed = mode == 1 || (mode >= 3 && mode <= 5);
        assert(result.status == (failed ? PXR_TASK_ERROR : PXR_TASK_OK));
        if (failed) assert(result.error == ETIMEDOUT);
        if (mode == 4) {
            char interim[25];
            assert(recv(pair[1], interim, sizeof(interim), MSG_WAITALL) == 25);
            assert(!memcmp(interim, "HTTP/1.1 100 Continue\r\n\r\n", 25));
        }
        errno = 0; assert(fcntl(closed_fd, F_GETFD) == -1 && errno == EBADF);
        int reused = open("/dev/null", O_RDONLY); assert(reused >= 0);
        pxr_gc_collect(); assert(fcntl(reused, F_GETFD) >= 0); close(reused);
        close(pair[1]); pxr_loop_destroy(&pxr_compiler_loop); pxr_compiler_loop_initialized = 0;
        pxr_gc_collect(); assert(!pxr_heap_blocks && !pxr_heap_bytes && !pxr_async_loops);
    }
}
int main(void) {
    alarm(20);
    parser_cases(); chunk_cases(); teardown_cases();
    alarm(0); puts("http clean");
}
