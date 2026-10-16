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

static void test_array_index(void) {
    pxr_gc_frame frame; void *roots[3] = {0}; pxr_gc_enter(&frame, roots); pxr_gc_roots(&frame, 3);
    pxr_array *a = pxr_array_new(), *keys = pxr_array_new(); roots[0] = a; roots[1] = keys;
    for (int i = 0; i < 512; i++) {
        pxr_str *key = pxr_str_alloc(6);
        key->data[0] = 'k'; key->data[1] = 0;
        memcpy(key->data + 2, &i, sizeof(i));
        pxr_array_push(keys, PXR_TAG_STR, (intptr_t)key);
    }
    int seen[1024] = {0}, snapshot_seen[1024] = {0}, order[1024], count = 0;
    int64_t expected[1024] = {0}, snapshot[1024] = {0};
    uint32_t random = 12345; pxr_array *copy = NULL;
    for (int step = 0; step < 20000; step++) {
        random = random * UINT32_C(1664525) + UINT32_C(1013904223);
        int id = (int)((random >> 10) & 1023);
        int64_t tag = id < 512 ? PXR_TAG_INT : PXR_TAG_STR;
        int64_t number = id - 256;
        pxr_str *key = id < 512 ? NULL : (void *)(intptr_t)keys->ents[id - 512].bits;
        if (random & 3) {
            if (!seen[id]) {seen[id] = 1; order[count++] = id;}
            expected[id] = step + 1;
            pxr_array_set(a, tag, number, key, PXR_TAG_INT, expected[id]);
        }
        /* Byte equality must work even when lookup uses a distinct allocation. */
        if (key && step % 31 == 0) {
            pxr_str *other = pxr_str_alloc(key->len); memcpy(other->data, key->data, key->len); key = other;
        }
        assert(pxr_array_has(a, tag, number, key) == seen[id]);
        assert(pxr_array_get_bits(a, tag, number, key) == expected[id]);
        assert(a->len == (size_t)count);
        if (step == 9999) {
            copy = pxr_array_copy(a); roots[2] = copy;
            memcpy(snapshot, expected, sizeof(expected)); memcpy(snapshot_seen, seen, sizeof(seen));
        }
        if (step % 257 == 0) pxr_gc_collect();
    }
    for (int i = 0; i < count; i++) {
        int id = order[i]; const pxr_ent *entry = &a->ents[i];
        if (id < 512) assert(!entry->ks && entry->ki == id - 256);
        else assert(pxr_str_eq(entry->ks, (void *)(intptr_t)keys->ents[id - 512].bits));
        assert(entry->bits == expected[id]);
    }
    for (int id = 0; id < 1024; id++) {
        int64_t tag = id < 512 ? PXR_TAG_INT : PXR_TAG_STR;
        pxr_str *key = id < 512 ? NULL : (void *)(intptr_t)keys->ents[id - 512].bits;
        assert(pxr_array_has(copy, tag, id - 256, key) == snapshot_seen[id]);
        assert(pxr_array_get_bits(copy, tag, id - 256, key) == snapshot[id]);
    }
    /* An intentionally crowded bucket exercises probe chains through growth,
     * missing lookups and overwrites, without changing insertion order. */
    a = pxr_array_new(); roots[0] = a;
    int64_t collisions[33]; int found = 0;
    for (int64_t key = 1; found < 33; key++)
        if (!(pxr_array_key_hash(PXR_TAG_INT, key, NULL) & 63)) collisions[found++] = key;
    for (int i = 0; i < 32; i++) pxr_array_set(a, PXR_TAG_INT, collisions[i], NULL, PXR_TAG_INT, i + 1);
    pxr_array_set(a, PXR_TAG_INT, collisions[17], NULL, PXR_TAG_INT, 999);
    for (int i = 0; i < 32; i++) {
        assert(a->ents[i].ki == collisions[i]);
        assert(pxr_array_get_bits(a, PXR_TAG_INT, collisions[i], NULL) == (i == 17 ? 999 : i + 1));
    }
    assert(!pxr_array_has(a, PXR_TAG_INT, collisions[32], NULL));
    pxr_gc_collect(); assert(pxr_array_get_bits(a, PXR_TAG_INT, collisions[17], NULL) == 999);
    /* Append metadata survives copies and the dense-to-indexed transition. */
    a = pxr_array_new(); roots[0] = a;
    for (int i = 0; i < 1000; i++) pxr_array_push(a, PXR_TAG_INT, i);
    assert(a->packed && !a->index && a->next_index == 1000);
    pxr_array_set(a, PXR_TAG_INT, -10, NULL, PXR_TAG_INT, 9);
    copy = pxr_array_copy(a); roots[2] = copy;
    pxr_array_push(copy, PXR_TAG_INT, 42);
    assert(pxr_array_get_bits(copy, PXR_TAG_INT, 1000, NULL) == 42);
    assert(!pxr_array_has(a, PXR_TAG_INT, 1000, NULL));
    a = pxr_array_new(); roots[0] = a;
    pxr_array_set(a, PXR_TAG_INT, INT64_MIN, NULL, PXR_TAG_INT, 1);
    pxr_array_push(a, PXR_TAG_INT, 2);
    assert(pxr_array_get_bits(a, PXR_TAG_INT, INT64_MIN + 1, NULL) == 2);
    pxr_array_set(a, PXR_TAG_INT, INT64_MAX - 1, NULL, PXR_TAG_INT, 3);
    pxr_array_push(a, PXR_TAG_INT, 4);
    assert(pxr_array_get_bits(a, PXR_TAG_INT, INT64_MAX, NULL) == 4 && a->append_overflow);
    pxr_gc_roots(&frame, 0); pxr_gc_collect(); assert(!pxr_heap_bytes && !pxr_heap_blocks);
    pxr_gc_leave(&frame);
}

static void test_array_cow(void) {
    pxr_gc_frame frame; void *roots[5] = {0};
    pxr_gc_enter(&frame, roots); pxr_gc_roots(&frame, 5);
    pxr_array *original = pxr_array_new(); roots[0] = original;
    for (int i = 0; i < 1000; i++) pxr_array_push(original, PXR_TAG_INT, i + 1);
    pxr_ent *backing = original->ents;
    size_t bytes = pxr_heap_bytes;
    pxr_array *copy = pxr_array_copy(original); roots[1] = copy;
    assert(copy != original && copy->ents == backing);
    assert(pxr_heap_bytes == bytes + sizeof(pxr_array));
    assert(((pxr_allocation *)backing - 1)->h.array_owners == 2);
    pxr_array_set(copy, PXR_TAG_INT, 0, NULL, PXR_TAG_INT, 99);
    assert(copy->ents != backing && original->ents == backing);
    assert(pxr_array_get_bits(original, PXR_TAG_INT, 0, NULL) == 1);
    assert(pxr_array_get_bits(copy, PXR_TAG_INT, 0, NULL) == 99);
    assert(((pxr_allocation *)backing - 1)->h.array_owners == 1);

    /* Reclaiming dead headers restores exclusive ownership without making the
     * surviving array pay an unnecessary separation on its next mutation. */
    copy = pxr_array_copy(original); roots[1] = NULL;
    assert(((pxr_allocation *)backing - 1)->h.array_owners == 2);
    pxr_gc_collect();
    assert(((pxr_allocation *)backing - 1)->h.array_owners == 1);
    pxr_array_push(original, PXR_TAG_INT, 1001);
    assert(original->ents == backing);

    /* A header/value graph with nested arrays and mixed boxes must retain its
     * descendants when only the shallow copy remains rooted. */
    pxr_array *child = pxr_array_new(); roots[2] = child;
    pxr_array_push(child, PXR_TAG_INT, 7);
    pxr_array *parent = pxr_array_new(); roots[0] = parent;
    pxr_array_push(parent, PXR_TAG_ARR, (intptr_t)pxr_array_copy(child));
    pxr_array_push(parent, PXR_TAG_MIXED, (intptr_t)pxr_mixed_box(PXR_TAG_ARR, (intptr_t)child));
    copy = pxr_array_copy(parent); roots[1] = copy;
    pxr_array_prepare_write(copy);
    pxr_array *inner = (void *)(intptr_t)pxr_array_get_bits(copy, PXR_TAG_INT, 0, NULL);
    pxr_array_set(inner, PXR_TAG_INT, 0, NULL, PXR_TAG_INT, 42);
    pxr_array *old_inner = (void *)(intptr_t)pxr_array_get_bits(parent, PXR_TAG_INT, 0, NULL);
    assert(pxr_array_get_bits(old_inner, PXR_TAG_INT, 0, NULL) == 7);
    assert(pxr_array_get_bits(inner, PXR_TAG_INT, 0, NULL) == 42);
    pxr_mixed *mixed = (void *)(intptr_t)pxr_array_get_bits(copy, PXR_TAG_INT, 1, NULL);
    pxr_array *mixed_child = (void *)(intptr_t)mixed->bits;
    pxr_array_set(mixed_child, PXR_TAG_INT, 0, NULL, PXR_TAG_MIXED, (intptr_t)pxr_mixed_box(PXR_TAG_INT, 99));
    pxr_mixed *old_mixed = (void *)(intptr_t)pxr_array_get_bits(parent, PXR_TAG_INT, 1, NULL);
    pxr_mixed *old_value = (void *)(intptr_t)pxr_array_get_bits((void *)(intptr_t)old_mixed->bits, PXR_TAG_INT, 0, NULL);
    assert(old_value->tag == PXR_TAG_INT && old_value->bits == 7);
    roots[0] = roots[2] = NULL; pxr_gc_collect();
    assert(pxr_array_get_bits(inner, PXR_TAG_INT, 0, NULL) == 42);
    pxr_mixed *new_value = (void *)(intptr_t)pxr_array_get_bits(mixed_child, PXR_TAG_INT, 0, NULL);
    assert(new_value->bits == 99);

    /* Copies share an existing string-key index and separate it before growth.
     * Objects stay shared, as PHP requires, even when array buffers separate. */
    original = pxr_array_new(); roots[0] = original;
    pxr_str *key = pxr_str_alloc(3); memcpy(key->data, "k\0x", 3);
    pxr_object *object = pxr_object_new(1, NULL, 0); roots[2] = object;
    pxr_array_set(original, PXR_TAG_STR, 0, key, PXR_TAG_OBJ, (intptr_t)object);
    copy = pxr_array_copy(original); roots[1] = copy;
    assert(copy->index == original->index && copy->ents == original->ents);
    pxr_array_push(copy, PXR_TAG_INT, 10);
    assert(copy->index != original->index && copy->ents != original->ents);
    assert(pxr_array_get_bits(copy, PXR_TAG_STR, 0, key) == (intptr_t)object);
    assert(original->len == 1 && copy->len == 2);
    pxr_gc_collect();
    assert(pxr_array_get_bits(original, PXR_TAG_STR, 0, key) == (intptr_t)object);

    /* An allocation guard can root a backing buffer independently of the
     * header: backing buffers themselves must trace all live entry payloads. */
    pxr_gc_guard guard = {pxr_gc_guards, pxr_heap_serial, NULL}; pxr_gc_guards = &guard;
    pxr_array *late = pxr_array_new(); pxr_array_push(late, PXR_TAG_INT, 123);
    pxr_array_push(copy, PXR_TAG_ARR, (intptr_t)late);
    roots[0] = roots[1] = roots[2] = NULL;
    pxr_gc_collect();
    assert(pxr_array_get_bits(late, PXR_TAG_INT, 0, NULL) == 123);
    pxr_gc_guards = guard.previous;
    pxr_gc_roots(&frame, 0); pxr_gc_collect(); assert(!pxr_heap_bytes && !pxr_heap_blocks);
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
    if (strcmp(argv[1], "array-cow") == 0) { test_array_cow(); return 0; }
    if (strcmp(argv[1], "array-index") == 0) { test_array_index(); return 0; }
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
    assert(copy != array && copy->ents == ((pxr_array *)array)->ents);

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
