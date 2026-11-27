/*
 * Minimal JSON parser for the glTF JSON chunk.
 *
 * Scope: objects, arrays, strings (with common escapes), numbers (double),
 * true / false / null. It is not a validating parser; it trusts well-formed
 * glTF and stops at the first structural error. The output is a malloc'd
 * jv tree freed by jv_free. Sufficient for glTF 2.0's JSON, nothing more.
 */
#include "internal.h"

#include <stdlib.h>
#include <string.h>

typedef struct {
    const char *p;   /* cursor */
    const char *end; /* one past last byte */
    size_t err_off;  /* byte offset of last failure */
} parser;

static jv *parse_value(parser *ps);

static jv *jv_new(jkind k) {
    jv *v = calloc(1, sizeof(jv));
    if (v)
        v->kind = k;
    return v;
}

void jv_free(jv *v) {
    if (!v)
        return;
    switch (v->kind) {
    case J_STR:
        free(v->str.data);
        break;
    case J_ARR:
        for (size_t i = 0; i < v->arr.count; ++i)
            jv_free(v->arr.items[i]);
        free(v->arr.items);
        break;
    case J_OBJ:
        for (size_t i = 0; i < v->obj.count; ++i) {
            free(v->obj.keys[i]);
            jv_free(v->obj.vals[i]);
        }
        free(v->obj.keys);
        free(v->obj.vals);
        break;
    default:
        break;
    }
    free(v);
}

static void skip_ws(parser *ps) {
    while (ps->p < ps->end) {
        char c = *ps->p;
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r')
            ps->p++;
        else
            break;
    }
}

static jv *parse_string(parser *ps) {
    /* Assumes *ps->p == '"' on entry. */
    ps->p++;
    const char *start = ps->p;
    /* First pass: find the closing quote, honouring backslash escapes. */
    const char *q = start;
    while (q < ps->end && *q != '"') {
        if (*q == '\\') {
            q += 2;
            continue;
        }
        q++;
    }
    if (q >= ps->end) {
        ps->err_off = (size_t)(ps->end - ps->p);
        return nullptr;
    }

    /* Second pass: unescape into a heap buffer. */
    size_t cap = (size_t)(q - start) + 1;
    char *out = malloc(cap);
    if (!out)
        return nullptr;
    size_t n = 0;
    for (const char *s = start; s < q; ++s) {
        if (*s != '\\') {
            out[n++] = *s;
            continue;
        }
        s++;
        if (s >= q)
            break;
        switch (*s) {
        case '"':
            out[n++] = '"';
            break;
        case '\\':
            out[n++] = '\\';
            break;
        case '/':
            out[n++] = '/';
            break;
        case 'b':
            out[n++] = '\b';
            break;
        case 'f':
            out[n++] = '\f';
            break;
        case 'n':
            out[n++] = '\n';
            break;
        case 'r':
            out[n++] = '\r';
            break;
        case 't':
            out[n++] = '\t';
            break;
        case 'u':
            /* \uXXXX: keep it simple — copy the raw bytes; glTF does not
             * rely on non-ASCII escapes in the fields this loader reads. */
            out[n++] = '\\';
            out[n++] = 'u';
            for (int i = 0; i < 4 && s + 1 < q; ++i) {
                s++;
                out[n++] = *s;
            }
            break;
        default:
            out[n++] = *s;
            break;
        }
    }
    out[n] = '\0';
    ps->p = q + 1; /* past closing quote */

    jv *v = jv_new(J_STR);
    if (!v) {
        free(out);
        return nullptr;
    }
    v->str.data = out;
    v->str.len = n;
    return v;
}

static jv *parse_number(parser *ps) {
    /* strtod handles sign, integer, fraction, exponent. */
    char *endp = nullptr;
    double d = strtod(ps->p, &endp);
    if (endp == ps->p) {
        ps->err_off = (size_t)(ps->p - ps->p);
        return nullptr;
    }
    jv *v = jv_new(J_NUM);
    if (!v)
        return nullptr;
    v->num = d;
    ps->p = endp;
    return v;
}

static bool match_lit(parser *ps, const char *lit, size_t n) {
    if ((size_t)(ps->end - ps->p) < n)
        return false;
    if (memcmp(ps->p, lit, n) != 0)
        return false;
    ps->p += n;
    return true;
}

static jv *parse_array(parser *ps) {
    ps->p++; /* '[' */
    jv *v = jv_new(J_ARR);
    if (!v)
        return nullptr;
    size_t cap = 0;
    skip_ws(ps);
    if (ps->p < ps->end && *ps->p == ']') {
        ps->p++;
        return v;
    }
    for (;;) {
        jv *item = parse_value(ps);
        if (!item)
            goto fail;
        if (v->arr.count == cap) {
            /* calloc: slots beyond count stay NULL, so jv_free's walk is
             * provably initialized even if a future edit loosens the
             * count coupling (the analyzer flags the realloc form). */
            size_t nc = cap ? cap * 2 : 8;
            jv **np = realloc(v->arr.items, nc * sizeof(jv *));
            if (!np) {
                jv_free(item);
                goto fail;
            }
            memset(np + v->arr.count, 0, (nc - v->arr.count) * sizeof(jv *));
            v->arr.items = np;
            cap = nc;
        }
        v->arr.items[v->arr.count++] = item;
        skip_ws(ps);
        if (ps->p >= ps->end)
            goto fail;
        if (*ps->p == ',') {
            ps->p++;
            skip_ws(ps);
            continue;
        }
        if (*ps->p == ']') {
            ps->p++;
            return v;
        }
        goto fail;
    }
fail:
    jv_free(v);
    return nullptr;
}

static jv *parse_object(parser *ps) {
    ps->p++; /* '{' */
    jv *v = jv_new(J_OBJ);
    if (!v)
        return nullptr;
    size_t cap = 0;
    skip_ws(ps);
    if (ps->p < ps->end && *ps->p == '}') {
        ps->p++;
        return v;
    }
    for (;;) {
        skip_ws(ps);
        if (ps->p >= ps->end || *ps->p != '"')
            goto fail;
        jv *key = parse_string(ps);
        if (!key)
            goto fail;
        skip_ws(ps);
        if (ps->p >= ps->end || *ps->p != ':') {
            jv_free(key);
            goto fail;
        }
        ps->p++;
        jv *val = parse_value(ps);
        if (!val) {
            jv_free(key);
            goto fail;
        }
        if (v->obj.count == cap) {
            size_t nc = cap ? cap * 2 : 8;
            char **nk = realloc(v->obj.keys, nc * sizeof(char *));
            jv **nv = realloc(v->obj.vals, nc * sizeof(jv *));
            if (!nk || !nv) {
                if (nk)
                    v->obj.keys = nk;
                if (nv)
                    v->obj.vals = nv;
                jv_free(key);
                jv_free(val);
                goto fail;
            }
            v->obj.keys = nk;
            v->obj.vals = nv;
            cap = nc;
        }
        v->obj.keys[v->obj.count] = key->str.data; /* steal the buffer */
        key->kind = J_NULL;                        /* don't double-free */
        jv_free(key);
        v->obj.vals[v->obj.count] = val;
        v->obj.count++;
        skip_ws(ps);
        if (ps->p >= ps->end)
            goto fail;
        if (*ps->p == ',') {
            ps->p++;
            continue;
        }
        if (*ps->p == '}') {
            ps->p++;
            return v;
        }
        goto fail;
    }
fail:
    jv_free(v);
    return nullptr;
}

static jv *parse_value(parser *ps) {
    skip_ws(ps);
    if (ps->p >= ps->end) {
        ps->err_off = (size_t)(ps->p - ps->p);
        return nullptr;
    }
    char c = *ps->p;
    if (c == '"')
        return parse_string(ps);
    if (c == '{')
        return parse_object(ps);
    if (c == '[')
        return parse_array(ps);
    if (c == 't') {
        if (match_lit(ps, "true", 4)) {
            jv *v = jv_new(J_BOOL);
            if (v)
                v->b = true;
            return v;
        }
        return nullptr;
    }
    if (c == 'f') {
        if (match_lit(ps, "false", 5)) {
            jv *v = jv_new(J_BOOL);
            if (v)
                v->b = false;
            return v;
        }
        return nullptr;
    }
    if (c == 'n') {
        if (match_lit(ps, "null", 4))
            return jv_new(J_NULL);
        return nullptr;
    }
    return parse_number(ps);
}

jv *jv_parse(const char *json, size_t len, size_t *err_off) {
    parser ps = {.p = json, .end = json + len, .err_off = 0};
    jv *v = parse_value(&ps);
    if (!v) {
        if (err_off)
            *err_off = ps.err_off;
        return nullptr;
    }
    /* Tolerate trailing whitespace only. */
    skip_ws(&ps);
    if (ps.p != ps.end) {
        if (err_off)
            *err_off = (size_t)(ps.p - json);
        jv_free(v);
        return nullptr;
    }
    return v;
}

const jv *jv_obj_get(const jv *v, const char *key) {
    if (!v || v->kind != J_OBJ)
        return nullptr;
    for (size_t i = 0; i < v->obj.count; ++i)
        if (strcmp(v->obj.keys[i], key) == 0)
            return v->obj.vals[i];
    return nullptr;
}

const jv *jv_arr_at(const jv *v, size_t i) {
    if (!v || v->kind != J_ARR || i >= v->arr.count)
        return nullptr;
    return v->arr.items[i];
}

double jv_num(const jv *v, double fallback) {
    if (!v || v->kind != J_NUM)
        return fallback;
    return v->num;
}

bool jv_bool(const jv *v, bool fallback) {
    if (!v || v->kind != J_BOOL)
        return fallback;
    return v->b;
}
