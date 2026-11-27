/*
 * .glb (binary glTF 2.0) container parser + glTF JSON walker.
 *
 * Resolves the supported subset (see scene-graph.h) into gfx mesh resources
 * and a node tree with world matrices. The JSON is parsed by json.c into a jv
 * tree; this file only walks it. Little-endian is assumed for the binary
 * container and accessor data (the glTF spec mandates both).
 */
#include "internal.h"

#include <optics/gfx/math.h>
#include <tessera/scene/render.h>

#include <float.h>
#include <limits.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ */
/*  Little-endian readers                                             */
/* ------------------------------------------------------------------ */

static uint32_t rd_u32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

/* ------------------------------------------------------------------ */
/*  .glb container                                                    */
/* ------------------------------------------------------------------ */

#define GLB_MAGIC 0x46546C67u /* "glTF" */
#define GLB_VERSION2 2u
#define CHUNK_JSON 0x4E4F534Au /* "JSON" */
#define CHUNK_BIN 0x004E4942u  /* "BIN\0" */

optics_gfx_result sg_glb_parse(const void *bytes, size_t len, sg_glb *out) {
    const uint8_t *p = bytes;
    if (len < 12)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    uint32_t magic = rd_u32(p);
    uint32_t version = rd_u32(p + 4);
    uint32_t total = rd_u32(p + 8);
    /* `total` is fully attacker-controlled: clamp it to the buffer
     * before any pointer arithmetic derives from it. A total < 12
     * (or > len) means no room for even the header + one chunk header;
     * the unsigned subtraction in the chunk loop would otherwise wrap
     * and walk past the buffer (found by fuzz_glb_parse in 90 seconds:
     * total=0 crashed rd_u32 reading out-of-bounds chunk headers). */
    if (magic != GLB_MAGIC || version != GLB_VERSION2)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (total < 12 || total > len)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;

    size_t offset = 12;
    memset(out, 0, sizeof(*out));
    while (offset + 8 <= (size_t)total) {
        const uint8_t *q = p + offset;
        uint32_t chunk_len = rd_u32(q);
        uint32_t ctype = rd_u32(q + 4);
        offset += 8;
        if ((size_t)chunk_len > (size_t)total - offset)
            return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
        if (ctype == CHUNK_JSON && !out->json) {
            out->json = p + offset;
            out->json_len = chunk_len;
        } else if (ctype == CHUNK_BIN && !out->bin) {
            out->bin = p + offset;
            out->bin_len = chunk_len;
        }
        offset += chunk_len;
    }
    if (offset != total)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (!out->json)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    return OPTICS_GFX_OK;
}

/* ------------------------------------------------------------------ */
/*  Accessor resolution                                               */
/* ------------------------------------------------------------------ */

static size_t comp_size(int comp) {
    switch (comp) {
    case SG_CT_BYTE:
    case SG_CT_UBYTE:
        return 1;
    case SG_CT_SHORT:
    case SG_CT_USHORT:
        return 2;
    case SG_CT_UINT:
    case SG_CT_FLOAT:
        return 4;
    default:
        return 0;
    }
}

static bool json_size(const jv *v, size_t fallback, size_t *out) {
    double n = v ? jv_num(v, -1.0) : (double)fallback;
    if (!isfinite(n) || n < 0.0 || n >= (double)SIZE_MAX)
        return false;
    size_t value = (size_t)n;
    if ((double)value != n)
        return false;
    *out = value;
    return true;
}

/* Resolve accessor `idx` to a fully bounds-checked span in its bufferView.
 * Returns NULL for malformed data as well as unsupported accessor forms. */
const uint8_t *sg_accessor_data(const jv *root, int idx, const uint8_t *bin, size_t bin_len,
                                int *comp, int *comps, size_t *count, size_t *stride,
                                bool *normalized) {
    if (idx < 0)
        return nullptr;
    const jv *accs = jv_obj_get(root, "accessors");
    const jv *bvws = jv_obj_get(root, "bufferViews");
    const jv *acc = jv_arr_at(accs, (size_t)idx);
    if (!acc)
        return nullptr;

    size_t bv_idx;
    if (!json_size(jv_obj_get(acc, "bufferView"), SIZE_MAX, &bv_idx) || bv_idx == SIZE_MAX)
        return nullptr;
    const jv *bv = jv_arr_at(bvws, bv_idx);
    if (!bv)
        return nullptr;

    size_t buf_idx;
    if (!json_size(jv_obj_get(bv, "buffer"), 0, &buf_idx) || buf_idx > INT_MAX)
        return nullptr;
    int buf = (int)buf_idx;
    size_t bv_off, bv_len, bv_stride, a_off;
    if (!json_size(jv_obj_get(bv, "byteOffset"), 0, &bv_off) ||
        !json_size(jv_obj_get(bv, "byteLength"), 0, &bv_len) ||
        !json_size(jv_obj_get(bv, "byteStride"), 0, &bv_stride) ||
        !json_size(jv_obj_get(acc, "byteOffset"), 0, &a_off))
        return nullptr;

    const uint8_t *seg = nullptr;
    size_t seglen = 0;
    if (buf == 0 && bin) {
        seg = bin;
        seglen = bin_len;
    }
    if (!seg || bv_off > seglen || bv_len > seglen - bv_off || a_off > bv_len)
        return nullptr;

    size_t comp_value;
    if (!json_size(jv_obj_get(acc, "componentType"), 0, &comp_value) || comp_value > INT_MAX)
        return nullptr;
    *comp = (int)comp_value;
    size_t csize = comp_size(*comp);
    if (csize == 0 || !json_size(jv_obj_get(acc, "count"), 0, count) || *count == 0)
        return nullptr;

    const char *type = nullptr;
    const jv *tv = jv_obj_get(acc, "type");
    if (tv && tv->kind == J_STR)
        type = tv->str.data;
    if (type && strcmp(type, "SCALAR") == 0)
        *comps = 1;
    else if (type && strcmp(type, "VEC2") == 0)
        *comps = 2;
    else if (type && strcmp(type, "VEC3") == 0)
        *comps = 3;
    else if (type && strcmp(type, "VEC4") == 0)
        *comps = 4;
    else if (type && strcmp(type, "MAT4") == 0)
        *comps = 16;
    else
        return nullptr;

    if (normalized)
        *normalized = jv_bool(jv_obj_get(acc, "normalized"), false);

    size_t element_size = csize * (size_t)*comps;
    if (bv_stride == 0)
        bv_stride = element_size;
    if (bv_stride < element_size)
        return nullptr;

    size_t available = bv_len - a_off;
    if (element_size > available || *count - 1 > (available - element_size) / bv_stride)
        return nullptr;

    *stride = bv_stride;
    return seg + bv_off + a_off;
}

/* Read one accessor element as up to 4 floats (normalising integer types to
 * float in [0,1] for normalised colour/UV channels — glTF normalised is not
 * yet honoured; integers are converted directly). */
void sg_read_floats(const uint8_t *ptr, int comp, int comps, bool normalized, float *out) {
    for (int i = 0; i < comps; ++i) {
        const uint8_t *e = ptr + (size_t)i * comp_size(comp);
        switch (comp) {
        case SG_CT_FLOAT: {
            uint32_t bits = rd_u32(e);
            memcpy(&out[i], &bits, sizeof(bits));
            break;
        }
        case SG_CT_USHORT: {
            uint16_t value = (uint16_t)e[0] | ((uint16_t)e[1] << 8);
            out[i] = normalized ? (float)value / 65535.0f : (float)value;
            break;
        }
        case SG_CT_UINT:
            out[i] = (float)rd_u32(e);
            break;
        case SG_CT_UBYTE:
            out[i] = normalized ? (float)*e / 255.0f : (float)*e;
            break;
        case SG_CT_BYTE: {
            int8_t value = (int8_t)*e;
            out[i] = normalized ? fmaxf((float)value / 127.0f, -1.0f) : (float)value;
            break;
        }
        case SG_CT_SHORT: {
            uint16_t bits = (uint16_t)e[0] | ((uint16_t)e[1] << 8);
            int16_t value;
            memcpy(&value, &bits, sizeof(value));
            out[i] = normalized ? fmaxf((float)value / 32767.0f, -1.0f) : (float)value;
            break;
        }
        default:
            out[i] = 0.0f;
            break;
        }
    }
}

/* ------------------------------------------------------------------ */
/*  Primitive build                                                   */
/* ------------------------------------------------------------------ */

static const jv *attr(const jv *prim, const char *name) {
    return jv_obj_get(jv_obj_get(prim, "attributes"), name);
}

/* Parse one primitive's geometry into device-independent form. Returns
 * true on success (ownership of the buffers passes to *out_data); false
 * on malformed/unsupported input (no allocation retained). */
static bool parse_primitive(const jv *root, const jv *prim, const uint8_t *bin, size_t bin_len,
                            sg_primitive_data *out_data) {
    int pos_acc = (int)jv_num(attr(prim, "POSITION"), -1);
    int nrm_acc = (int)jv_num(attr(prim, "NORMAL"), -1);
    int uv_acc = (int)jv_num(attr(prim, "TEXCOORD_0"), -1);
    int joint_acc = (int)jv_num(attr(prim, "JOINTS_0"), -1);
    int weight_acc = (int)jv_num(attr(prim, "WEIGHTS_0"), -1);

    int pcomp = 0, pcomps = 0;
    size_t pcount = 0, pstride = 0;
    bool pnormalized = false;
    const uint8_t *pos_ptr = pos_acc >= 0
                                 ? sg_accessor_data(root, pos_acc, bin, bin_len, &pcomp, &pcomps,
                                                    &pcount, &pstride, &pnormalized)
                                 : nullptr;
    if (!pos_ptr || pcomp != SG_CT_FLOAT || pcomps != 3 || pcount == 0)
        return false;

    int ncomp = 0, ncomps = 0;
    size_t ncount = 0, nstride = 0;
    bool nnormalized = false;
    const uint8_t *nrm_ptr = nrm_acc >= 0
                                 ? sg_accessor_data(root, nrm_acc, bin, bin_len, &ncomp, &ncomps,
                                                    &ncount, &nstride, &nnormalized)
                                 : nullptr;
    bool have_n = nrm_ptr && ncomp == SG_CT_FLOAT && ncomps == 3 && ncount == pcount;

    int ucomp = 0, ucomps = 0;
    size_t ucount = 0, ustride = 0;
    bool unormalized = false;
    const uint8_t *uv_ptr = uv_acc >= 0 ? sg_accessor_data(root, uv_acc, bin, bin_len, &ucomp,
                                                           &ucomps, &ucount, &ustride, &unormalized)
                                        : nullptr;
    bool have_uv = uv_ptr && ucomp == SG_CT_FLOAT && ucomps == 2 && ucount == pcount;

    int jcomp = 0, jcomps = 0, wcomp = 0, wcomps = 0;
    size_t jcount = 0, jstride = 0, wcount = 0, wstride = 0;
    bool jnormalized = false, wnormalized = false;
    const uint8_t *joint_ptr = joint_acc >= 0
                                   ? sg_accessor_data(root, joint_acc, bin, bin_len, &jcomp,
                                                      &jcomps, &jcount, &jstride, &jnormalized)
                                   : nullptr;
    const uint8_t *weight_ptr = weight_acc >= 0
                                    ? sg_accessor_data(root, weight_acc, bin, bin_len, &wcomp,
                                                       &wcomps, &wcount, &wstride, &wnormalized)
                                    : nullptr;
    bool have_skin = joint_ptr && weight_ptr && jcomps == 4 && wcomps == 4 && jcount == pcount &&
                     wcount == pcount && (jcomp == SG_CT_UBYTE || jcomp == SG_CT_USHORT) &&
                     (wcomp == SG_CT_FLOAT || wcomp == SG_CT_UBYTE || wcomp == SG_CT_USHORT);

    if (pcount > UINT32_MAX || pcount > SIZE_MAX / sizeof(tessera_scene_vertex))
        return false;
    tessera_scene_vertex *verts = malloc(pcount * sizeof(tessera_scene_vertex));
    if (!verts)
        return false;
    tessera_scene_skin_vertex *skin_verts = have_skin ? malloc(pcount * sizeof(*skin_verts)) : nullptr;
    if (have_skin && !skin_verts) {
        free(verts);
        return false;
    }
    optics_gfx_vec3 aabb_min = {FLT_MAX, FLT_MAX, FLT_MAX};
    optics_gfx_vec3 aabb_max = {-FLT_MAX, -FLT_MAX, -FLT_MAX};
    for (size_t i = 0; i < pcount; ++i) {
        const uint8_t *pp = pos_ptr + i * pstride;
        float p[3] = {0, 0, 0};
        sg_read_floats(pp, SG_CT_FLOAT, 3, false, p);
        verts[i].position[0] = p[0];
        verts[i].position[1] = p[1];
        verts[i].position[2] = p[2];
        if (p[0] < aabb_min.x)
            aabb_min.x = p[0];
        if (p[1] < aabb_min.y)
            aabb_min.y = p[1];
        if (p[2] < aabb_min.z)
            aabb_min.z = p[2];
        if (p[0] > aabb_max.x)
            aabb_max.x = p[0];
        if (p[1] > aabb_max.y)
            aabb_max.y = p[1];
        if (p[2] > aabb_max.z)
            aabb_max.z = p[2];

        if (have_n) {
            float n[3] = {0, 0, 0};
            sg_read_floats(nrm_ptr + i * nstride, SG_CT_FLOAT, 3, false, n);
            verts[i].normal[0] = n[0];
            verts[i].normal[1] = n[1];
            verts[i].normal[2] = n[2];
        } else {
            verts[i].normal[0] = verts[i].normal[1] = verts[i].normal[2] = 0.0f;
        }
        if (have_uv) {
            float uv[2] = {0, 0};
            sg_read_floats(uv_ptr + i * ustride, SG_CT_FLOAT, 2, false, uv);
            verts[i].uv[0] = uv[0];
            verts[i].uv[1] = uv[1];
        } else {
            verts[i].uv[0] = verts[i].uv[1] = 0.0f;
        }
        if (have_skin) {
            float joints[4] = {}, weights[4] = {};
            sg_read_floats(joint_ptr + i * jstride, jcomp, 4, false, joints);
            sg_read_floats(weight_ptr + i * wstride, wcomp, 4, wnormalized, weights);
            float total = 0.0f;
            for (int k = 0; k < 4; ++k) {
                if (joints[k] < 0.0f || joints[k] > 65535.0f) {
                    free(skin_verts);
                    free(verts);
                    return false;
                }
                skin_verts[i].joints[k] = (uint16_t)joints[k];
                skin_verts[i].weights[k] = fmaxf(weights[k], 0.0f);
                total += skin_verts[i].weights[k];
            }
            if (total > FLT_EPSILON) {
                for (int k = 0; k < 4; ++k)
                    skin_verts[i].weights[k] /= total;
            } else {
                skin_verts[i].weights[0] = 1.0f;
                skin_verts[i].weights[1] = 0.0f;
                skin_verts[i].weights[2] = 0.0f;
                skin_verts[i].weights[3] = 0.0f;
            }
        }
    }

    /* Indices (optional). Generated sequentially when absent. */
    uint32_t *idx = nullptr;
    size_t idx_count = 0;
    int idx_acc = (int)jv_num(jv_obj_get(prim, "indices"), -1);
    if (idx_acc >= 0) {
        int icomp = 0, icomps = 0;
        size_t icount = 0, istride = 0;
        const uint8_t *ip = sg_accessor_data(root, idx_acc, bin, bin_len, &icomp, &icomps, &icount,
                                             &istride, nullptr);
        if (!ip || icomps != 1 || icount == 0) {
            free(skin_verts);
            free(verts);
            return false;
        }
        if (icomp != SG_CT_UBYTE && icomp != SG_CT_USHORT && icomp != SG_CT_UINT) {
            free(skin_verts);
            free(verts);
            return false;
        }
        if (icount > UINT32_MAX || icount > SIZE_MAX / sizeof(uint32_t)) {
            free(skin_verts);
            free(verts);
            return false;
        }
        idx = malloc(icount * sizeof(uint32_t));
        if (!idx) {
            free(skin_verts);
            free(verts);
            return false;
        }
        for (size_t i = 0; i < icount; ++i) {
            const uint8_t *value = ip + i * istride;
            idx[i] = icomp == SG_CT_UBYTE    ? value[0]
                     : icomp == SG_CT_USHORT ? ((uint32_t)value[0] | ((uint32_t)value[1] << 8))
                                             : rd_u32(value);
            if (idx[i] >= pcount) {
                free(idx);
                free(skin_verts);
                free(verts);
                return false;
            }
        }
        idx_count = icount;
    } else {
        idx_count = pcount;
        if (idx_count > SIZE_MAX / sizeof(uint32_t)) {
            free(skin_verts);
            free(verts);
            return false;
        }
        idx = malloc(idx_count * sizeof(uint32_t));
        if (!idx) {
            free(skin_verts);
            free(verts);
            return false;
        }
        for (size_t i = 0; i < idx_count; ++i)
            idx[i] = (uint32_t)i;
    }

    out_data->vertices = verts;
    out_data->vertex_count = (uint32_t)pcount;
    out_data->indices = idx;
    out_data->index_count = (uint32_t)idx_count;
    out_data->skin_vertices = skin_verts;
    out_data->aabb_min = aabb_min;
    out_data->aabb_max = aabb_max;
    out_data->base_color = optics_gfx_vec4_make(1.0f, 1.0f, 1.0f, 1.0f);
    out_data->material_index = -1;

    /* Map the glTF material's base colour, if any. */
    int mat_idx = (int)jv_num(jv_obj_get(prim, "material"), -1);
    out_data->material_index = mat_idx;
    const jv *mats = jv_obj_get(root, "materials");
    const jv *mat = jv_arr_at(mats, (size_t)mat_idx);
    const jv *pbr = mat ? jv_obj_get(mat, "pbrMetallicRoughness") : nullptr;
    const jv *bcf = pbr ? jv_obj_get(pbr, "baseColorFactor") : nullptr;
    if (bcf && bcf->kind == J_ARR && bcf->arr.count >= 4) {
        out_data->base_color = optics_gfx_vec4_make(
            (float)jv_num(jv_arr_at(bcf, 0), 1.0), (float)jv_num(jv_arr_at(bcf, 1), 1.0),
            (float)jv_num(jv_arr_at(bcf, 2), 1.0), (float)jv_num(jv_arr_at(bcf, 3), 1.0));
    }
    return true;
}

/* ------------------------------------------------------------------ */
/*  Node tree + world matrices                                        */
/* ------------------------------------------------------------------ */

static optics_gfx_quat quat_from_rotation_columns(const float r[9]) {
    optics_gfx_quat q;
    float trace = r[0] + r[4] + r[8];
    if (trace > 0.0f) {
        float s = sqrtf(trace + 1.0f) * 2.0f;
        q = (optics_gfx_quat){(r[7] - r[5]) / s, (r[2] - r[6]) / s, (r[3] - r[1]) / s, 0.25f * s};
    } else if (r[0] > r[4] && r[0] > r[8]) {
        float s = sqrtf(1.0f + r[0] - r[4] - r[8]) * 2.0f;
        q = (optics_gfx_quat){0.25f * s, (r[1] + r[3]) / s, (r[2] + r[6]) / s, (r[7] - r[5]) / s};
    } else if (r[4] > r[8]) {
        float s = sqrtf(1.0f + r[4] - r[0] - r[8]) * 2.0f;
        q = (optics_gfx_quat){(r[1] + r[3]) / s, 0.25f * s, (r[5] + r[7]) / s, (r[2] - r[6]) / s};
    } else {
        float s = sqrtf(1.0f + r[8] - r[0] - r[4]) * 2.0f;
        q = (optics_gfx_quat){(r[2] + r[6]) / s, (r[5] + r[7]) / s, 0.25f * s, (r[3] - r[1]) / s};
    }
    return optics_gfx_quat_normalize(q);
}

static void decompose_matrix(optics_gfx_mat4 m, optics_gfx_vec3 *t, optics_gfx_quat *q,
                             optics_gfx_vec3 *s) {
    *t = optics_gfx_vec3_make(m.m[12], m.m[13], m.m[14]);
    s->x = sqrtf(m.m[0] * m.m[0] + m.m[1] * m.m[1] + m.m[2] * m.m[2]);
    s->y = sqrtf(m.m[4] * m.m[4] + m.m[5] * m.m[5] + m.m[6] * m.m[6]);
    s->z = sqrtf(m.m[8] * m.m[8] + m.m[9] * m.m[9] + m.m[10] * m.m[10]);
    if (s->x < FLT_EPSILON || s->y < FLT_EPSILON || s->z < FLT_EPSILON) {
        *q = optics_gfx_quat_identity();
        return;
    }
    optics_gfx_vec3 c0 = optics_gfx_vec3_make(m.m[0] / s->x, m.m[1] / s->x, m.m[2] / s->x);
    optics_gfx_vec3 c1 = optics_gfx_vec3_make(m.m[4] / s->y, m.m[5] / s->y, m.m[6] / s->y);
    optics_gfx_vec3 c2 = optics_gfx_vec3_make(m.m[8] / s->z, m.m[9] / s->z, m.m[10] / s->z);
    if (optics_gfx_vec3_dot(optics_gfx_vec3_cross(c0, c1), c2) < 0.0f) {
        s->x = -s->x;
        c0 = optics_gfx_vec3_scale(c0, -1.0f);
    }
    /* quat_from_rotation_columns consumes a row-major 3x3 matrix.  glTF and
     * optics_gfx_mat4 store the basis vectors as columns, so transpose while
     * flattening instead of accidentally interpreting the inverse rotation. */
    float r[9] = {c0.x, c1.x, c2.x, c0.y, c1.y, c2.y, c0.z, c1.z, c2.z};
    *q = quat_from_rotation_columns(r);
}

void sg_read_node(const jv *node, tessera_scene_node *n) {
    n->parent = -1;
    n->mesh_first = -1;
    n->skin = -1;
    n->rest_translation = optics_gfx_vec3_make(0, 0, 0);
    n->rest_rotation = optics_gfx_quat_identity();
    n->rest_scale = optics_gfx_vec3_make(1, 1, 1);
    const jv *m = jv_obj_get(node, "matrix");
    if (m && m->kind == J_ARR && m->arr.count >= 16) {
        optics_gfx_mat4 matrix;
        for (int i = 0; i < 16; ++i)
            matrix.m[i] = (float)jv_num(jv_arr_at(m, (size_t)i), 0.0);
        decompose_matrix(matrix, &n->rest_translation, &n->rest_rotation, &n->rest_scale);
    } else {
        const jv *tv = jv_obj_get(node, "translation");
        if (tv && tv->kind == J_ARR && tv->arr.count >= 3)
            n->rest_translation = optics_gfx_vec3_make((float)jv_num(jv_arr_at(tv, 0), 0),
                                                       (float)jv_num(jv_arr_at(tv, 1), 0),
                                                       (float)jv_num(jv_arr_at(tv, 2), 0));
        const jv *qv = jv_obj_get(node, "rotation");
        if (qv && qv->kind == J_ARR && qv->arr.count >= 4)
            n->rest_rotation = optics_gfx_quat_normalize((optics_gfx_quat){
                (float)jv_num(jv_arr_at(qv, 0), 0), (float)jv_num(jv_arr_at(qv, 1), 0),
                (float)jv_num(jv_arr_at(qv, 2), 0), (float)jv_num(jv_arr_at(qv, 3), 1)});
        const jv *sv = jv_obj_get(node, "scale");
        if (sv && sv->kind == J_ARR && sv->arr.count >= 3)
            n->rest_scale = optics_gfx_vec3_make((float)jv_num(jv_arr_at(sv, 0), 1),
                                                 (float)jv_num(jv_arr_at(sv, 1), 1),
                                                 (float)jv_num(jv_arr_at(sv, 2), 1));
    }
    n->translation = n->rest_translation;
    n->rotation = n->rest_rotation;
    n->scale = n->rest_scale;
    n->world = optics_gfx_mat4_identity();
    n->rest_world_rotation = optics_gfx_quat_identity();
    const jv *name = jv_obj_get(node, "name");
    if (name && name->kind == J_STR) {
        n->name = malloc(name->str.len + 1);
        if (n->name)
            memcpy(n->name, name->str.data, name->str.len + 1);
    }
}

/* ------------------------------------------------------------------ */
/*  Public: parse .glb into a scene                                   */
/* ------------------------------------------------------------------ */

optics_gfx_result sg_parse_glb(const void *bytes, size_t len, sg_scene_data *data) {
    sg_glb g;
    optics_gfx_result r = sg_glb_parse(bytes, len, &g);
    if (r != OPTICS_GFX_OK)
        return r;

    /* Declared up front and NULL/zero-initialised so the single `fail`
     * block at the bottom can release them from any exit (the repo's
     * standard single-exit discipline; see window app init for the
     * large-scale version). */
    int *roots = nullptr;
    uint32_t root_n = 0;
    tessera_scene_node *narr = nullptr;
    tessera_scene_skin *skin_arr = nullptr;
    uint32_t skin_count = 0;
    size_t prim_cap = 16, prim_n = 0;
    sg_primitive_data *prims = nullptr;
    int *mesh_prim_start = nullptr, *mesh_prim_count = nullptr;
    jv *root = nullptr;

    size_t err = 0;
    root = jv_parse((const char *)g.json, g.json_len, &err);
    if (!root || root->kind != J_OBJ) {
        jv_free(root);
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    }

    const jv *meshes = jv_obj_get(root, "meshes");
    const jv *nodes = jv_obj_get(root, "nodes");
    uint32_t mesh_count = (meshes && meshes->kind == J_ARR) ? (uint32_t)meshes->arr.count : 0;
    uint32_t node_count = (nodes && nodes->kind == J_ARR) ? (uint32_t)nodes->arr.count : 0;

    /* First pass: parse all primitives, record each mesh's primitive span. */
    mesh_prim_start = mesh_count ? calloc(mesh_count + 1, sizeof(int)) : nullptr;
    mesh_prim_count = mesh_count ? calloc(mesh_count, sizeof(int)) : nullptr;
    if (mesh_count && (!mesh_prim_start || !mesh_prim_count)) {
        r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
        goto fail;
    }

    prims = malloc(prim_cap * sizeof(*prims));
    if (!prims) {
        r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
        goto fail;
    }
    memset(prims, 0, prim_cap * sizeof(*prims));

    for (uint32_t mi = 0; mi < mesh_count; ++mi) {
        const jv *mesh = jv_arr_at(meshes, mi);
        const jv *prims_arr = jv_obj_get(mesh, "primitives");
        uint32_t pc = (prims_arr && prims_arr->kind == J_ARR) ? (uint32_t)prims_arr->arr.count : 0;
        mesh_prim_start[mi] = (int)prim_n;
        for (uint32_t pi = 0; pi < pc; ++pi) {
            if (prim_n == prim_cap) {
                prim_cap *= 2;
                sg_primitive_data *np = realloc(prims, prim_cap * sizeof(*prims));
                if (!np) {
                    r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
                    goto fail;
                }
                memset(np + prim_n, 0, (prim_cap - prim_n) * sizeof(*prims));
                prims = np;
            }
            if (!parse_primitive(root, jv_arr_at(prims_arr, pi), g.bin, g.bin_len,
                                 &prims[prim_n])) {
                continue; /* unsupported primitive — skip, not fatal */
            }
            prim_n++;
        }
        mesh_prim_count[mi] = (int)(prim_n - (size_t)mesh_prim_start[mi]);
    }

    if (prim_n == 0) {
        r = OPTICS_GFX_ERROR_UNSUPPORTED;
        goto fail;
    }

    /* Nodes. */
    if (node_count) {
        narr = calloc(node_count, sizeof(*narr));
        if (!narr) {
            r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
            goto fail;
        }
        for (uint32_t ni = 0; ni < node_count; ++ni) {
            tessera_scene_node *n = &narr[ni];
            const jv *node = jv_arr_at(nodes, ni);
            sg_read_node(node, n);
            int mi = (int)jv_num(jv_obj_get(node, "mesh"), -1);
            if (mi >= 0 && (uint32_t)mi < mesh_count) {
                n->mesh_first = mesh_prim_start[mi];
                n->mesh_prim_count = mesh_prim_count[mi];
            }
            n->skin = (int)jv_num(jv_obj_get(node, "skin"), -1);
        }
        /* Resolve parent links from `children`. */
        for (uint32_t ni = 0; ni < node_count; ++ni) {
            const jv *node = jv_arr_at(nodes, ni);
            const jv *ch = jv_obj_get(node, "children");
            if (!ch || ch->kind != J_ARR)
                continue;
            for (size_t c = 0; c < ch->arr.count; ++c) {
                int ci = (int)jv_num(jv_arr_at(ch, c), -1);
                if (ci >= 0 && (uint32_t)ci < node_count)
                    narr[ci].parent = (int)ni;
            }
        }
    }

    /* Skins: joint node indices plus inverse bind matrices. Palettes are kept
     * CPU-side and refreshed whenever the pose changes; draw copies them into
     * the frame transient ring for GPU skinning. */
    const jv *skins = jv_obj_get(root, "skins");
    skin_count = skins && skins->kind == J_ARR ? (uint32_t)skins->arr.count : 0;
    if (skin_count) {
        skin_arr = calloc(skin_count, sizeof(*skin_arr));
        if (!skin_arr) {
            r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
            goto fail;
        }
        for (uint32_t si = 0; si < skin_count; ++si) {
            const jv *skin_json = jv_arr_at(skins, si);
            const jv *joints = jv_obj_get(skin_json, "joints");
            if (!joints || joints->kind != J_ARR || joints->arr.count == 0 ||
                joints->arr.count > UINT32_MAX) {
                r = OPTICS_GFX_ERROR_INVALID_ARGUMENT;
                goto fail;
            }
            tessera_scene_skin *skin = &skin_arr[si];
            skin->joint_count = (uint32_t)joints->arr.count;
            skin->joints = malloc(skin->joint_count * sizeof(*skin->joints));
            skin->inverse_bind = malloc(skin->joint_count * sizeof(*skin->inverse_bind));
            skin->palette = malloc(skin->joint_count * sizeof(*skin->palette));
            if (!skin->joints || !skin->inverse_bind || !skin->palette) {
                r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
                goto fail;
            }
            for (uint32_t ji = 0; ji < skin->joint_count; ++ji) {
                int node_index = (int)jv_num(jv_arr_at(joints, ji), -1);
                if (node_index < 0 || (uint32_t)node_index >= node_count) {
                    r = OPTICS_GFX_ERROR_INVALID_ARGUMENT;
                    goto fail;
                }
                skin->joints[ji] = node_index;
                skin->inverse_bind[ji] = optics_gfx_mat4_identity();
            }
            int ibm_acc = (int)jv_num(jv_obj_get(skin_json, "inverseBindMatrices"), -1);
            if (ibm_acc >= 0) {
                int comp = 0, comps = 0;
                size_t count = 0, stride = 0;
                bool normalized = false;
                const uint8_t *ibm = sg_accessor_data(root, ibm_acc, g.bin, g.bin_len, &comp,
                                                      &comps, &count, &stride, &normalized);
                if (!ibm || comp != SG_CT_FLOAT || comps != 16 || count != skin->joint_count) {
                    r = OPTICS_GFX_ERROR_INVALID_ARGUMENT;
                    goto fail;
                }
                for (uint32_t ji = 0; ji < skin->joint_count; ++ji)
                    sg_read_floats(ibm + (size_t)ji * stride, comp, 16, false,
                                   skin->inverse_bind[ji].m);
            }
        }
    }

    for (uint32_t ni = 0; ni < node_count; ++ni)
        if (narr[ni].skin < 0 || (uint32_t)narr[ni].skin >= skin_count)
            narr[ni].skin = -1;

    /* Roots: the default scene's nodes, else every parentless node. */
    int scene_idx = (int)jv_num(jv_obj_get(root, "scene"), -1);
    const jv *scenes = jv_obj_get(root, "scenes");
    const jv *scene = jv_arr_at(scenes, (size_t)scene_idx);
    const jv *scene_nodes = scene ? jv_obj_get(scene, "nodes") : nullptr;
    if (scene_nodes && scene_nodes->kind == J_ARR) {
        root_n = (uint32_t)scene_nodes->arr.count;
        roots = root_n ? malloc(root_n * sizeof(int)) : nullptr;
        if (root_n && !roots) {
            r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
            goto fail;
        }
        for (uint32_t i = 0; i < root_n; ++i)
            roots[i] = (int)jv_num(jv_arr_at(scene_nodes, i), -1);
    } else if (node_count) {
        roots = malloc(node_count * sizeof(int));
        if (!roots) {
            r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
            goto fail;
        }
        for (uint32_t ni = 0; ni < node_count; ++ni)
            if (narr[ni].parent < 0 && root_n < node_count)
                roots[root_n++] = (int)ni;
    }

    /* Success: hand ownership of everything to the caller's data. */
    data->prims = prims;
    data->prim_count = (uint32_t)prim_n;
    data->nodes = narr;
    data->node_count = node_count;
    data->skins = skin_arr;
    data->skin_count = skin_count;
    data->roots = roots;
    data->root_count = root_n;
    for (int i = 0; i < SG_HUMAN_BONE_COUNT; ++i)
        data->human_bones[i] = -1;
    const jv *extensions = jv_obj_get(root, "extensions");
    if (jv_obj_get(extensions, "VRMC_vrm"))
        sg_read_humanoid(root, "VRMC_vrm", false, data->human_bones);
    else if (jv_obj_get(extensions, "VRM"))
        sg_read_humanoid(root, "VRM", true, data->human_bones);
    free(mesh_prim_start);
    free(mesh_prim_count);
    jv_free(root);
    return OPTICS_GFX_OK;

    /* ---- Single cleanup for every error exit above ----
     *
     * Every exit sets `r` in place and jumps here — no error-code-mapping
     * labels chained into `fail`. Ownership is never partially
     * transferred, so releasing the locals is always correct; `data`
     * itself is zeroed by the caller and stays owned by the caller on
     * failure. */
fail:
    if (prims)
        for (size_t i = 0; i < prim_n; ++i) {
            free(prims[i].vertices);
            free(prims[i].indices);
            free(prims[i].skin_vertices);
        }
    free(prims);
    if (narr)
        for (uint32_t i = 0; i < node_count; ++i)
            free(narr[i].name);
    free(narr);
    if (skin_arr)
        for (uint32_t i = 0; i < skin_count; ++i) {
            free(skin_arr[i].joints);
            free(skin_arr[i].inverse_bind);
            free(skin_arr[i].palette);
        }
    free(skin_arr);
    free(roots);
    free(mesh_prim_start);
    free(mesh_prim_count);
    jv_free(root);
    return r;
}
