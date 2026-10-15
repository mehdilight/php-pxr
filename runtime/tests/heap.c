/* Appended to the runtime source by the Rust integration test. */
#include <assert.h>
#include <limits.h>

const char pxr_php_worker_source[] = "";
pxr_str *pxr_native_object_to_string(void *object) {
    (void)object;
    assert(pxr_gc_guards != NULL);
    pxr_gc_collect(); /* must preserve the runtime snapshot during reentrancy */
    pxr_str *s = pxr_str_alloc(2);
    memcpy(s->data, "ok", 2);
    return s;
}

static void test_collection(void) {
    void *roots[2]; pxr_gc_frame frame;
    pxr_gc_enter(&frame, roots);
    pxr_str *key = pxr_str_alloc(3); memcpy(key->data, "a\0b", 3);
    pxr_array *array = pxr_array_new();
    pxr_object *object = pxr_object_new(1, key, 1);
    pxr_object_set_bits(object, 0, PXR_TAG_OBJ, (intptr_t)object);
    void *boxed = pxr_mixed_box(PXR_TAG_OBJ, (intptr_t)object);
    pxr_array_set(array, PXR_TAG_STR, 0, key, PXR_TAG_MIXED, (intptr_t)boxed);
    roots[0] = array; roots[1] = key; pxr_gc_roots(&frame, 2);
    size_t live = pxr_heap_bytes;
    for (int i = 0; i < 1000; i++) pxr_str_alloc(100);
    pxr_gc_collect();
    assert(pxr_heap_bytes == live);
    assert(pxr_array_get_bits(array, PXR_TAG_STR, 0, key) == (intptr_t)boxed);
    assert(pxr_mixed_str(boxed)->len == 2);
    /* Roots survive nested frames; ending a frame does not sweep its return. */
    pxr_gc_frame nested; void *nested_roots[] = { boxed };
    pxr_gc_enter(&nested, nested_roots); pxr_gc_roots(&nested, 1);
    pxr_gc_collect(); pxr_gc_leave(&nested);
    pxr_gc_roots(&frame, 0); pxr_gc_collect();
    assert(pxr_heap_blocks == 0 && pxr_heap_bytes == 0); /* cycle freed */

    /* Marking is iterative, so deep object chains cannot exhaust the C stack. */
    pxr_object *chain = NULL;
    for (int i = 0; i < 20000; i++) {
        pxr_object *next = pxr_object_new(1, NULL, 1);
        pxr_object_set_bits(next, 0, PXR_TAG_OBJ, (intptr_t)chain);
        chain = next;
    }
    roots[0] = chain; pxr_gc_roots(&frame, 1); pxr_gc_collect();
    assert(pxr_heap_blocks == 60000);
    pxr_gc_roots(&frame, 0); pxr_gc_collect();
    assert(pxr_heap_blocks == 0);

    /* Metadata/static storage are explicit global roots, handle caches weak. */
    pxr_class_register(1, 2);
    pxr_object *storage = pxr_static_storage(1, 1);
    key = pxr_str_alloc(4); memcpy(key->data, "live", 4);
    pxr_object_set_bits(storage, 0, PXR_TAG_STR, (intptr_t)key);
    pxr_php_handle *handle = pxr_heap_alloc(sizeof(*handle));
    handle->id = 1; handle->next = NULL;
    handle->object = pxr_object_new(-1, key, 0);
    pxr_php_handles = handle;
    roots[0] = handle->object; pxr_gc_roots(&frame, 1); pxr_gc_collect();
    assert(pxr_php_handles == handle);
    pxr_gc_roots(&frame, 0); pxr_gc_collect();
    assert(pxr_php_handles == NULL);
    assert(pxr_instanceof(storage, 2));
    assert(pxr_object_get_bits(storage, 0) == (intptr_t)key);
    assert(key->len == 4 && !memcmp(key->data, "live", 4));
    pxr_gc_leave(&frame);
}

static void assert_shutdown(void) {
    assert(pxr_heap_head == NULL);
    assert(pxr_heap_blocks == 0);
    assert(pxr_heap_bytes == 0);
    puts("heap clean");
}

int main(int argc, char **argv) {
    assert(argc == 2);
    /* Registered first so this runs AFTER the heap's exit callback. */
    assert(atexit(assert_shutdown) == 0);
    if (strcmp(argv[1], "collection") == 0) { test_collection(); return 0; }
    if (strcmp(argv[1], "alloc-overflow") == 0) pxr_heap_alloc(SIZE_MAX);
    if (strcmp(argv[1], "string-overflow") == 0) pxr_str_alloc(SIZE_MAX);
    if (strcmp(argv[1], "mul-overflow") == 0) pxr_size_mul(SIZE_MAX, 2);
    if (strcmp(argv[1], "add-overflow") == 0) pxr_size_add(SIZE_MAX, 1);
    if (strcmp(argv[1], "object-overflow") == 0) pxr_object_new(1, NULL, LLONG_MAX);
    if (strcmp(argv[1], "array-overflow") == 0) {
        pxr_array array = {0};
        pxr_array_grow(&array, SIZE_MAX);
    }
    if (strcmp(argv[1], "capacity") == 0) {
        assert(pxr_grow_capacity(SIZE_MAX / 2 + 1, SIZE_MAX) == SIZE_MAX);
    }
    void *zero = pxr_heap_alloc(0);
    assert((uintptr_t)zero % _Alignof(max_align_t) == 0);
    unsigned char *first = pxr_heap_alloc(32);
    unsigned char *middle = pxr_heap_alloc(64);
    unsigned char *last = pxr_heap_alloc(128);
    memset(first, 0x5a, 32);
    pxr_heap_free(middle); /* unlink middle */
    pxr_heap_free(last);   /* unlink head */
    first = pxr_heap_resize(first, 4096);
    for (size_t i = 0; i < 32; i++) assert(first[i] == 0x5a);
    first = pxr_heap_resize(first, 8);
    for (size_t i = 0; i < 8; i++) assert(first[i] == 0x5a);
    pxr_heap_free(zero); /* unlink tail */
    pxr_heap_free(first);
    pxr_heap_free(NULL);
    assert(pxr_heap_blocks == 0 && pxr_heap_bytes == 0);

    pxr_str *s = pxr_str_alloc(3);
    memcpy(s->data, "a\0b", 3);
    if (strcmp(argv[1], "repeat-overflow") == 0) pxr_str_repeat(s, LLONG_MAX);
    if (strcmp(argv[1], "fatal") == 0) pxr_str_repeat(s, -1);
    if (strcmp(argv[1], "format-fatal") == 0) {
        pxr_str *format = pxr_str_alloc(1);
        format->data[0] = '%';
        pxr_sprintf(format, pxr_array_new());
    }
    void *array = pxr_array_new();
    for (int i = 0; i < 20000; i++) pxr_array_push(array, PXR_TAG_STR, (intptr_t)s);
    assert(pxr_array_count(array) == 20000);
    assert(pxr_array_get_bits(array, PXR_TAG_INT, 19999, NULL) == (intptr_t)s);
    /* Resizing must release every obsolete buffer. */
    assert(pxr_heap_blocks == 3); /* string, array header, current entries */
    pxr_array *copy = pxr_array_copy(array);
    assert(copy != array && copy->ents != ((pxr_array *)array)->ents);

    pxr_object *object = pxr_object_new(1, s, 1);
    pxr_object_set_bits(object, 0, PXR_TAG_OBJ, (intptr_t)object); /* cycle */
    pxr_class_register(1, 2);
    assert(pxr_instanceof(object, 2));
    pxr_static_storage(1, 2);
    pxr_mixed_box(PXR_TAG_OBJ, (intptr_t)object);

    pxr_str *format = pxr_str_alloc(2);
    memcpy(format->data, "%s", 2);
    pxr_array *args = pxr_array_new();
    pxr_array_push(args, PXR_TAG_STR, (intptr_t)s);
    size_t before_format = pxr_heap_blocks;
    pxr_str *formatted = pxr_sprintf(format, args);
    assert(formatted->len == 3 && memcmp(formatted->data, s->data, 3) == 0);
    assert(pxr_heap_blocks == before_format + 1); /* result only, no scratch */

    size_t blocks = pxr_heap_blocks, bytes = pxr_heap_bytes;
    pxr_buf buffer;
    pxr_buf_init(&buffer, 1);
    for (int i = 0; i < 10000; i++) pxr_buf_append_byte(&buffer, 'x');
    assert(buffer.len == 10000 && buffer.data[9999] == 'x');
    pxr_heap_free(buffer.data);
    assert(pxr_heap_blocks == blocks && pxr_heap_bytes == bytes);
    return 0; /* exit callback reclaims the entire graph, including the cycle */
}
