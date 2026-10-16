/* Original bounded HTTP/1.x adapter for the structured server. Appended after
 * pxr_async.c. Protocol and compatibility boundaries: docs/HTTP.md. */
#include <ctype.h>

static pxr_str *pxr_http_string(const char *data, size_t length) {
    pxr_str *text = pxr_str_alloc(length);
    if (length) memcpy(text->data, data, length);
    return text;
}
static void pxr_http_put(pxr_array *array, pxr_str *key, int tag, intptr_t bits) {
    void *value = pxr_mixed_box(tag, bits);
    pxr_array_set(array, PXR_TAG_STR, 0, key, PXR_TAG_MIXED, (intptr_t)value);
}
static void pxr_http_text(pxr_array *array, const char *key, const char *data, size_t length) {
    pxr_http_put(array, pxr_http_string(key, strlen(key)), PXR_TAG_STR,
        (intptr_t)pxr_http_string(data, length));
}
static void pxr_http_literal(pxr_array *array, const char *key, const char *value) {
    pxr_http_text(array, key, value, strlen(value));
}
static void pxr_http_number(pxr_array *array, const char *key, int value) {
    char buffer[24]; int length = snprintf(buffer, sizeof(buffer), "%d", value);
    pxr_http_text(array, key, buffer, (size_t)length);
}
static void pxr_http_mark_context(pxr_http_context *context) {
    if (!context) return;
    pxr_gc_mark(context);
    for (size_t i = 0; i < 6; i++) pxr_gc_mark(context->globals[i]);
    pxr_gc_mark(context->headers); pxr_gc_mark(context->response_headers);
    pxr_gc_mark(context->body);
    pxr_gc_mark(context->output.data);
}
static pxr_http_context *pxr_http_context_new(int fd, int64_t max_bytes) {
    pxr_http_context *context = pxr_heap_alloc(sizeof(*context));
    memset(context, 0, sizeof(*context));
    for (size_t i = 0; i < 6; i++) context->globals[i] = pxr_array_new();
    context->headers = pxr_array_new(); context->response_headers = pxr_array_new();
    context->status = 200; context->max_bytes = max_bytes;
    if (fd >= 0) {
        pxr_array *server = context->globals[3];
        for (int peer = 0; peer < 2; peer++) {
            struct sockaddr_storage address; socklen_t size = sizeof(address);
            int result = peer ? getpeername(fd, (struct sockaddr *)&address, &size)
                              : getsockname(fd, (struct sockaddr *)&address, &size);
            if (result < 0) continue;
            char text[INET6_ADDRSTRLEN]; void *ip = NULL; int port = 0;
            if (address.ss_family == AF_INET) {
                struct sockaddr_in *v4 = (void *)&address; ip = &v4->sin_addr; port = ntohs(v4->sin_port);
            } else if (address.ss_family == AF_INET6) {
                struct sockaddr_in6 *v6 = (void *)&address; ip = &v6->sin6_addr; port = ntohs(v6->sin6_port);
            }
            if (ip && inet_ntop(address.ss_family, ip, text, sizeof(text)))
                pxr_http_literal(server, peer ? "REMOTE_ADDR" : "SERVER_ADDR", text);
            pxr_http_number(server, peer ? "REMOTE_PORT" : "SERVER_PORT", port);
        }
        pxr_http_literal(server, "SERVER_SOFTWARE", "PXR");
    }
    return context;
}
static pxr_http_context *pxr_http_current(int required) {
    pxr_task *task = pxr_compiler_loop.current;
    if (task && task->http) return task->http;
    if (required) { fputs("pxr: HTTP function requires a native HTTP request\n", stderr); exit(255); }
    if (!pxr_http_cli_context) pxr_http_cli_context = pxr_http_context_new(-1, 0);
    return pxr_http_cli_context;
}
static int pxr_http_output(const char *data, size_t length) {
    pxr_task *task = pxr_compiler_loop.current;
    if (!task || !task->http) return 0;
    pxr_http_context *context = task->http;
    if (!context->error) {
        if (length > (uint64_t)context->max_bytes - context->output.len) context->error = EMSGSIZE;
        else pxr_buf_append(&context->output, data, length);
    }
    return 1;
}
void *pxr_http_global(int64_t id) {
    assert(id >= 0 && id < 6);
    return pxr_http_current(0)->globals[id];
}
pxr_str *pxr_http_body(void) { return pxr_http_current(1)->body; }
int64_t pxr_http_status(int64_t code) {
    pxr_http_context *context = pxr_http_current(1);
    int previous = context->status;
    if (code && (code < 200 || code > 599)) context->error = EINVAL;
    else if (code) context->status = (int)code;
    return previous;
}
static int pxr_http_token(unsigned char c) {
    return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') ||
        (c && strchr("!#$%&'*+-.^_`|~", c));
}
static unsigned char pxr_http_lower(unsigned char c) { return c >= 'A' && c <= 'Z' ? c + 32 : c; }
static int pxr_http_equal(const char *a, size_t length, const char *literal) {
    if (length != strlen(literal)) return 0;
    for (size_t i = 0; i < length; i++) if (pxr_http_lower((unsigned char)a[i]) != (unsigned char)literal[i]) return 0;
    return 1;
}
static int pxr_http_value(const char *data, size_t length) {
    for (size_t i = 0; i < length; i++) {
        unsigned char c = (unsigned char)data[i];
        if ((c < 32 && c != '\t') || c == 127) return 0;
    }
    return 1;
}
/* Header entries are raw strings in a private ordered array, so Set-Cookie and
 * other repeated fields retain their order rather than being overwritten. */
void pxr_http_header(pxr_str *line, int64_t replace, int64_t code) {
    pxr_http_context *context = pxr_http_current(1);
    if (code) pxr_http_status(code);
    if (!line->len || !pxr_http_value(line->data, line->len)) { context->error = EINVAL; return; }
    if (line->len >= 12 && (pxr_http_equal(line->data, 9, "http/1.0 ") || pxr_http_equal(line->data, 9, "http/1.1 "))) {
        if (line->data[9] < '2' || line->data[9] > '5' || line->data[10] < '0' || line->data[10] > '9' ||
            line->data[11] < '0' || line->data[11] > '9' || (line->len > 12 && line->data[12] != ' ')) {
            context->error = EINVAL; return;
        }
        pxr_http_status((line->data[9] - '0') * 100 + (line->data[10] - '0') * 10 + line->data[11] - '0');
        return;
    }
    const char *colon = memchr(line->data, ':', line->len);
    size_t name = colon ? (size_t)(colon - line->data) : 0;
    if (!name) { context->error = EINVAL; return; }
    for (size_t i = 0; i < name; i++) if (!pxr_http_token((unsigned char)line->data[i])) { context->error = EINVAL; return; }
    if (pxr_http_equal(line->data, name, "content-length") || pxr_http_equal(line->data, name, "transfer-encoding") ||
        pxr_http_equal(line->data, name, "connection") || pxr_http_equal(line->data, name, "trailer") ||
        pxr_http_equal(line->data, name, "upgrade")) { context->error = EINVAL; return; }
    pxr_array *headers = context->response_headers;
    if (replace) for (size_t i = 0; i < headers->len;) {
        pxr_str *old = (void *)(intptr_t)headers->ents[i].bits;
        const char *old_colon = memchr(old->data, ':', old->len);
        size_t old_name = old_colon ? (size_t)(old_colon - old->data) : 0;
        int same = old_name == name;
        for (size_t j = 0; same && j < name; j++) same = pxr_http_lower((unsigned char)old->data[j]) == pxr_http_lower((unsigned char)line->data[j]);
        if (!same) { i++; continue; }
        memmove(headers->ents + i, headers->ents + i + 1, (headers->len - i - 1) * sizeof(pxr_ent)); headers->len--;
    }
    size_t length = line->len;
    for (size_t i = 0; i < headers->len; i++) length = pxr_size_add(length, ((pxr_str *)(intptr_t)headers->ents[i].bits)->len + 2);
    if (headers->len >= 100 || length > 16384) { context->error = EMSGSIZE; return; }
    pxr_array_push(headers, PXR_TAG_STR, (intptr_t)line);
    if (pxr_http_equal(line->data, name, "location") && context->status != 201 &&
        (context->status < 300 || context->status > 399)) context->status = 302;
}
static const char *pxr_http_reason(int status) {
    switch (status) {
        case 200: return "OK"; case 201: return "Created"; case 202: return "Accepted";
        case 204: return "No Content"; case 205: return "Reset Content";
        case 301: return "Moved Permanently"; case 302: return "Found"; case 303: return "See Other";
        case 304: return "Not Modified"; case 307: return "Temporary Redirect"; case 308: return "Permanent Redirect";
        case 400: return "Bad Request"; case 401: return "Unauthorized"; case 403: return "Forbidden";
        case 404: return "Not Found"; case 405: return "Method Not Allowed"; case 408: return "Request Timeout";
        case 411: return "Length Required"; case 413: return "Content Too Large"; case 414: return "URI Too Long";
        case 415: return "Unsupported Media Type"; case 417: return "Expectation Failed";
        case 422: return "Unprocessable Content"; case 429: return "Too Many Requests";
        case 431: return "Request Header Fields Too Large"; case 500: return "Internal Server Error";
        case 501: return "Not Implemented"; case 503: return "Service Unavailable";
        case 505: return "HTTP Version Not Supported"; default: return "Response";
    }
}
static void pxr_http_response(pxr_task *task, pxr_server_connection *frame, pxr_str *body, int status) {
    pxr_http_context *context = task->http;
    if (!status && (context->error || body->len > (uint64_t)context->max_bytes - context->output.len)) status = 500;
    int generated_error = status != 0;
    if (status) {
        context->response_headers->len = 0;
        const char *reason = pxr_http_reason(status);
        body = pxr_str_alloc(strlen(reason) + 1);
        memcpy(body->data, reason, body->len - 1); body->data[body->len - 1] = '\n';
    } else status = context->status;
    pxr_buf response = {0};
    char line[128];
    int length = snprintf(line, sizeof(line), "HTTP/1.1 %d %s\r\nConnection: close\r\n", status, pxr_http_reason(status));
    pxr_buf_append(&response, line, (size_t)length);
    int no_content = status == 204 || status == 304;
    size_t prefix = generated_error ? 0 : context->output.len;
    size_t body_length = status == 205 || no_content ? 0 : pxr_size_add(prefix, body->len);
    if (!no_content) {
        length = snprintf(line, sizeof(line), "Content-Length: %zu\r\n", body_length);
        pxr_buf_append(&response, line, (size_t)length);
    }
    int content_type = 0, date_header = 0;
    pxr_array *headers = context->response_headers;
    for (size_t i = 0; i < headers->len; i++) {
        pxr_str *header = (void *)(intptr_t)headers->ents[i].bits;
        const char *colon = memchr(header->data, ':', header->len);
        size_t name = (size_t)(colon - header->data);
        if (pxr_http_equal(header->data, name, "content-type")) content_type = 1;
        if (pxr_http_equal(header->data, name, "date")) date_header = 1;
        pxr_buf_append(&response, header->data, header->len); pxr_buf_append(&response, "\r\n", 2);
    }
    if (!content_type) {
        const char *header = "Content-Type: text/html; charset=UTF-8\r\n";
        pxr_buf_append(&response, header, strlen(header));
    }
    if (!date_header) {
        time_t now = time(NULL); struct tm time;
        if (gmtime_r(&now, &time)) {
            size_t n = strftime(line, sizeof(line), "Date: %a, %d %b %Y %H:%M:%S GMT\r\n", &time);
            pxr_buf_append(&response, line, n);
        }
    }
    pxr_buf_append(&response, "\r\n", 2);
    if (!context->head && body_length) {
        if (prefix) pxr_buf_append(&response, context->output.data, prefix);
        pxr_buf_append(&response, body->data, body->len);
    }
    frame->response = pxr_http_string(response.data, response.len);
    task->roots[1] = frame->response;
    pxr_heap_free(response.data);
    frame->phase = PXR_SERVER_WRITE;
}
static int pxr_http_error(pxr_task *task, pxr_server_connection *frame, int status) {
    pxr_http_response(task, frame, NULL, status);
    task->roots[0] = NULL; pxr_heap_free(frame->request.data); frame->request.data = NULL;
    return 2;
}
static int pxr_http_hex(unsigned char c) {
    if (c >= '0' && c <= '9') return c - '0';
    c = pxr_http_lower(c); return c >= 'a' && c <= 'f' ? c - 'a' + 10 : -1;
}
static pxr_str *pxr_http_decode(const char *data, size_t length, int plus) {
    pxr_str *text = pxr_str_alloc(length); size_t out = 0;
    for (size_t i = 0; i < length; i++) {
        if (data[i] == '%' && i + 2 < length && pxr_http_hex((unsigned char)data[i + 1]) >= 0 && pxr_http_hex((unsigned char)data[i + 2]) >= 0) {
            text->data[out++] = (char)(pxr_http_hex((unsigned char)data[i + 1]) * 16 + pxr_http_hex((unsigned char)data[i + 2])); i += 2;
        } else text->data[out++] = data[i] == '+' && plus ? ' ' : data[i];
    }
    text->len = out; text->data[out] = 0; return text;
}
static int pxr_http_register(pxr_array *array, pxr_str *name, pxr_str *value, int cookie) {
    /* PHP-compatible common bracket forms, numeric keys, leading spaces and
     * dot/space name mangling. Malformed brackets fail, never flatten silently. */
    size_t start = 0, end = 0;
    while (start < name->len && name->data[start] == ' ') start++;
    while (end < name->len && name->data[end]) end++;
    int original_host = end - start >= 7 && !memcmp(name->data + start, "__Host-", 7);
    int original_secure = end - start >= 9 && !memcmp(name->data + start, "__Secure-", 9);
    size_t top = start;
    while (top < end && name->data[top] != '[') {
        if (name->data[top] == '.' || name->data[top] == ' ') name->data[top] = '_';
        top++;
    }
    if (top == start) return 1;
    pxr_str *keys[17]; int append[17] = {0}; size_t count = 1;
    keys[0] = pxr_http_string(name->data + start, top - start);
    if (cookie && ((!original_host && keys[0]->len >= 7 && !memcmp(keys[0]->data, "__Host-", 7)) ||
                   (!original_secure && keys[0]->len >= 9 && !memcmp(keys[0]->data, "__Secure-", 9)))) return 1;
    size_t at = top;
    while (at < end) {
        if (count == 17 || name->data[at++] != '[') return 0;
        size_t first = at;
        while (at < end && name->data[at] != ']') at++;
        if (at == end) return 0;
        append[count] = at == first || (at - first == 1 && isspace((unsigned char)name->data[first]));
        keys[count++] = pxr_http_string(name->data + first, at - first); at++;
        if (at < end && name->data[at] != '[') return 0;
    }
    for (size_t i = 0; i < count; i++) {
        pxr_str *key = keys[i];
        long long ktag = PXR_TAG_STR, ki = 0; const pxr_str *ks = key;
        if (append[i]) {
            ki = 0;
            for (size_t j = 0; j < array->len; j++) if (!array->ents[j].ks && array->ents[j].ki >= ki) {
                if (array->ents[j].ki == INT64_MAX) return 0;
                ki = array->ents[j].ki + 1;
            }
            ktag = PXR_TAG_INT; ks = NULL;
        } else pxr_norm_key(&ktag, &ki, &ks);
        long index = pxr_array_find(array, ktag, ki, ks);
        if (i + 1 == count) {
            if (cookie && index >= 0) return 1;
            pxr_array_set(array, ktag, ki, ks, PXR_TAG_MIXED,
                (intptr_t)pxr_mixed_box(PXR_TAG_STR, (intptr_t)value));
        } else {
            pxr_array *child = NULL;
            if (index >= 0) {
                const pxr_mixed *old = pxr_mixed_checked((void *)(intptr_t)array->ents[index].bits);
                if (old->tag == PXR_TAG_ARR) child = (void *)(intptr_t)old->bits;
            }
            if (!child) {
                child = pxr_array_new();
                /* This fresh internal array must be stored by identity while
                 * filling it. Ordinary mixed boxing deliberately copies arrays. */
                pxr_mixed *boxed = pxr_heap_alloc(sizeof(*boxed));
                ((pxr_allocation *)boxed - 1)->h.kind = PXR_HEAP_MIXED;
                boxed->tag = PXR_TAG_ARR; boxed->bits = (intptr_t)child;
                pxr_array_set(array, ktag, ki, ks, PXR_TAG_MIXED,
                    (intptr_t)boxed);
            }
            array = child;
        }
    }
    return 1;
}
static int pxr_http_parameters(pxr_array *array, const char *data, size_t length, int cookie) {
    size_t at = 0, count = 0;
    while (at < length) {
        size_t end = at; while (end < length && data[end] != (cookie ? ';' : '&')) end++;
        size_t start = at; at = end + 1;
        if (cookie) while (start < end && (data[start] == ' ' || data[start] == '\t')) start++;
        if (start == end) continue;
        if (++count > 1024) return 0;
        size_t equal = start; while (equal < end && data[equal] != '=') equal++;
        pxr_str *name = cookie ? pxr_http_string(data + start, equal - start) : pxr_http_decode(data + start, equal - start, 1);
        pxr_str *value = equal == end ? pxr_str_alloc(0) : pxr_http_decode(data + equal + 1, end - equal - 1, !cookie);
        if (!pxr_http_register(array, name, value, cookie)) return 0;
    }
    return 1;
}
static pxr_str *pxr_http_find(pxr_http_context *context, const char *name) {
    for (size_t i = 0; i < context->headers->len; i++) {
        pxr_ent *entry = &context->headers->ents[i];
        if (entry->ks && pxr_http_equal(entry->ks->data, entry->ks->len, name)) return (void *)(intptr_t)entry->bits;
    }
    return NULL;
}
static int pxr_http_host(const char *data, size_t length) {
    if (!length) return 0;
    size_t at = 0;
    if (data[0] == '[') {
        while (at < length && data[at] != ']') at++;
        if (at == length || at - 1 >= INET6_ADDRSTRLEN) return 0;
        char ip[INET6_ADDRSTRLEN]; memcpy(ip, data + 1, at - 1); ip[at - 1] = 0;
        struct in6_addr address; if (inet_pton(AF_INET6, ip, &address) != 1) return 0;
        at++;
    } else {
        while (at < length && data[at] != ':') {
            unsigned char c = (unsigned char)data[at++];
            if (!((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '.' || c == '-' || c == '_')) return 0;
        }
        if (!at) return 0;
    }
    if (at == length) return 1;
    if (data[at++] != ':' || at == length) return 0;
    unsigned port = 0;
    for (; at < length; at++) {
        if (data[at] < '0' || data[at] > '9') return 0;
        port = port * 10 + (unsigned)(data[at] - '0'); if (port > 65535) return 0;
    }
    return 1;
}
static int pxr_http_parse_headers(pxr_http_context *context, const char *data) {
    size_t line = 0;
    while (line + 1 < context->header_end && !(data[line] == '\r' && data[line + 1] == '\n')) line++;
    size_t method = 0; while (method < line && data[method] != ' ') method++;
    if (!method || method > 32 || method == line) return 400;
    for (size_t i = 0; i < method; i++) if (!pxr_http_token((unsigned char)data[i])) return 400;
    context->head = method == 4 && !memcmp(data, "HEAD", 4);
    size_t target = method + 1, space = target;
    while (space < line && data[space] != ' ') space++;
    if (space == target || space == line || data[target] != '/') return 400;
    if (space - target > 8192) return 414;
    for (size_t i = target; i < space; i++) {
        unsigned char c = (unsigned char)data[i];
        if (c <= 32 || c >= 127 || c == '#' || c == '\\') return 400;
        if (c == '%' && (i + 2 >= space || pxr_http_hex((unsigned char)data[i + 1]) < 0 || pxr_http_hex((unsigned char)data[i + 2]) < 0)) return 400;
    }
    int v11 = line - space == 9 && !memcmp(data + space + 1, "HTTP/1.1", 8);
    if (!v11 && !(line - space == 9 && !memcmp(data + space + 1, "HTTP/1.0", 8))) return 505;
    pxr_array *server = context->globals[3];
    pxr_http_text(server, "REQUEST_METHOD", data, method);
    pxr_http_text(server, "REQUEST_URI", data + target, space - target);
    pxr_http_text(server, "SERVER_PROTOCOL", data + space + 1, 8);
    size_t query = target; while (query < space && data[query] != '?') query++;
    pxr_http_text(server, "PATH_INFO", data + target, query - target);
    pxr_http_text(server, "QUERY_STRING", data + (query < space ? query + 1 : space), query < space ? space - query - 1 : 0);
    if (query < space && !pxr_http_parameters(context->globals[0], data + query + 1, space - query - 1, 0)) return 400;
    size_t at = line + 2, count = 0; int hosts = 0, lengths = 0, transfer = 0, expect = 0;
    while (at + 2 < context->header_end) {
        size_t end = at; while (end + 1 < context->header_end && !(data[end] == '\r' && data[end + 1] == '\n')) end++;
        size_t colon = at; while (colon < end && data[colon] != ':') colon++;
        if (colon == at || colon == end || ++count > 100) return count > 100 ? 431 : 400;
        for (size_t i = at; i < colon; i++) if (!pxr_http_token((unsigned char)data[i])) return 400;
        size_t value = colon + 1, last = end;
        while (value < last && (data[value] == ' ' || data[value] == '\t')) value++;
        while (last > value && (data[last - 1] == ' ' || data[last - 1] == '\t')) last--;
        if (!pxr_http_value(data + value, last - value)) return 400;
        pxr_str *key = pxr_http_string(data + at, colon - at);
        for (size_t i = 0; i < key->len; i++) key->data[i] = (char)pxr_http_lower((unsigned char)key->data[i]);
        if (pxr_http_equal(key->data, key->len, "host")) {
            if (++hosts > 1 || !pxr_http_host(data + value, last - value)) return 400;
        }
        if (pxr_http_equal(key->data, key->len, "content-length")) {
            if (++lengths > 1 || value == last) return 400;
            uint64_t size = 0;
            for (size_t i = value; i < last; i++) {
                if (data[i] < '0' || data[i] > '9') return 400;
                if (size > (UINT64_MAX - (unsigned)(data[i] - '0')) / 10) return 413;
                size = size * 10 + (unsigned)(data[i] - '0');
            }
            if (size > (uint64_t)context->max_bytes || size > SIZE_MAX - 16384) return 413;
            context->body_length = (size_t)size;
        }
        if (pxr_http_equal(key->data, key->len, "transfer-encoding")) transfer = 1;
        if (pxr_http_equal(key->data, key->len, "expect")) expect = 1;
        pxr_str *old = pxr_http_find(context, key->data);
        pxr_str *text;
        if (old) {
            text = pxr_str_alloc(pxr_size_add(old->len + 2, last - value));
            memcpy(text->data, old->data, old->len);
            memcpy(text->data + old->len, pxr_http_equal(key->data, key->len, "cookie") ? "; " : ", ", 2);
            memcpy(text->data + old->len + 2, data + value, last - value);
        } else text = pxr_http_string(data + value, last - value);
        pxr_array_set(context->headers, PXR_TAG_STR, 0, key, PXR_TAG_STR, (intptr_t)text);
        at = end + 2;
    }
    if (v11 && hosts != 1) return 400;
    if (transfer) return lengths ? 400 : 501;
    if (expect) return 417;
    if (pxr_http_find(context, "upgrade")) return 501;
    pxr_str *encoding = pxr_http_find(context, "content-encoding");
    if (encoding && !pxr_http_equal(encoding->data, encoding->len, "identity")) return 415;
    pxr_str *content_type = pxr_http_find(context, "content-type");
    if (content_type) {
        size_t n = 0; while (n < content_type->len && content_type->data[n] != ';' && content_type->data[n] != ' ') n++;
        if (pxr_http_equal(content_type->data, n, "multipart/form-data")) return 415;
    }
    for (size_t i = 0; i < context->headers->len; i++) {
        pxr_ent *entry = &context->headers->ents[i]; pxr_str *key = entry->ks;
        pxr_str *text = (void *)(intptr_t)entry->bits;
        if (pxr_http_equal(key->data, key->len, "content-type")) pxr_http_text(server, "CONTENT_TYPE", text->data, text->len);
        else if (pxr_http_equal(key->data, key->len, "content-length")) pxr_http_text(server, "CONTENT_LENGTH", text->data, text->len);
        else if (!pxr_http_equal(key->data, key->len, "proxy")) {
            pxr_str *name = pxr_str_alloc(key->len + 5); memcpy(name->data, "HTTP_", 5);
            for (size_t j = 0; j < key->len; j++) name->data[j + 5] = key->data[j] == '-' ? '_' : (key->data[j] >= 'a' && key->data[j] <= 'z' ? key->data[j] - 32 : key->data[j]);
            pxr_http_put(server, name, PXR_TAG_STR, (intptr_t)text);
        }
    }
    return 0;
}
static void pxr_http_merge(pxr_array *destination, pxr_array *source) {
    for (size_t i = 0; i < source->len; i++) {
        pxr_ent *entry = &source->ents[i];
        long old = pxr_array_find(destination, entry->ks ? PXR_TAG_STR : PXR_TAG_INT, entry->ki, entry->ks);
        const pxr_mixed *value = pxr_mixed_checked((void *)(intptr_t)entry->bits);
        const pxr_mixed *previous = old < 0 ? NULL : pxr_mixed_checked((void *)(intptr_t)destination->ents[old].bits);
        if (value->tag == PXR_TAG_ARR && previous && previous->tag == PXR_TAG_ARR)
            pxr_http_merge((void *)(intptr_t)previous->bits, (void *)(intptr_t)value->bits);
        else pxr_array_set(destination, entry->ks ? PXR_TAG_STR : PXR_TAG_INT, entry->ki, entry->ks,
            PXR_TAG_MIXED, (intptr_t)pxr_mixed_copy((void *)(intptr_t)entry->bits));
    }
}
static int pxr_http_consume(pxr_task *task, pxr_server_connection *frame, int eof) {
    pxr_http_context *context = task->http;
    if (!context->parsed) {
        size_t i = context->scan;
        for (; i + 3 < frame->request.len; i++) if (!memcmp(frame->request.data + i, "\r\n\r\n", 4)) break;
        if (i + 3 >= frame->request.len) {
            context->scan = frame->request.len > 3 ? frame->request.len - 3 : 0;
            if (frame->request.len >= 16384) return pxr_http_error(task, frame, 431);
            return eof ? pxr_http_error(task, frame, 400) : 0;
        }
        context->header_end = i + 4;
        int status = pxr_http_parse_headers(context, frame->request.data);
        if (status) return pxr_http_error(task, frame, status);
        context->parsed = 1;
    }
    if (frame->request.len - context->header_end < context->body_length)
        return eof ? pxr_http_error(task, frame, 400) : 0;
    context->body = pxr_http_string(frame->request.data + context->header_end, context->body_length);
    pxr_str *cookie = pxr_http_find(context, "cookie");
    if (cookie && !pxr_http_parameters(context->globals[2], cookie->data, cookie->len, 1)) return pxr_http_error(task, frame, 400);
    pxr_str *content_type = pxr_http_find(context, "content-type");
    pxr_str *method_key = pxr_http_string("REQUEST_METHOD", 14);
    void *method_box = (void *)(intptr_t)pxr_array_get_bits(context->globals[3], PXR_TAG_STR, 0, method_key);
    const pxr_mixed *method = pxr_mixed_checked(method_box);
    pxr_str *method_text = (void *)(intptr_t)method->bits;
    if (method_text->len == 4 && !memcmp(method_text->data, "POST", 4) && content_type) {
        size_t length = 0; while (length < content_type->len && content_type->data[length] != ';' && content_type->data[length] != ' ') length++;
        if (pxr_http_equal(content_type->data, length, "application/x-www-form-urlencoded") &&
            !pxr_http_parameters(context->globals[1], context->body->data, context->body->len, 0)) return pxr_http_error(task, frame, 400);
    }
    /* REQUEST uses fixed GET-then-POST precedence and owns an independent copy. */
    context->globals[4] = pxr_array_copy(context->globals[0]);
    pxr_http_merge(context->globals[4], context->globals[1]);
    return 1;
}
void *pxr_async_http_serve(pxr_str *address, int64_t port, pxr_http_handler handler,
                          int64_t max_requests, int64_t max_bytes,
                          int64_t timeout_ms, int64_t max_concurrent) {
    pxr_task *task = pxr_async_tcp_serve(address, port, NULL, max_requests, max_bytes, timeout_ms, max_concurrent);
    ((pxr_server_frame *)task->frame)->http_handler = handler;
    return task;
}
