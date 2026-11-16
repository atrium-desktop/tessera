// toplevel_drag_probe.c — verify xdg_toplevel_drag_manager_v1 and xdg_toplevel_drag_v1 binding
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wayland-client.h>
#include "xdg-toplevel-drag-v1-client.h"

static struct wl_display *g_display;
static struct wl_registry *g_registry;
static struct xdg_toplevel_drag_manager_v1 *g_drag_manager;
static struct wl_data_device_manager *g_ddm;

static void registry_global(void *data, struct wl_registry *registry,
                            uint32_t name, const char *interface, uint32_t version) {
    (void)data;
    if (strcmp(interface, xdg_toplevel_drag_manager_v1_interface.name) == 0) {
        g_drag_manager = wl_registry_bind(registry, name, &xdg_toplevel_drag_manager_v1_interface, 1);
        printf("toplevel-drag-manager-bound\n");
        fflush(stdout);
    } else if (strcmp(interface, "wl_data_device_manager") == 0) {
        g_ddm = wl_registry_bind(registry, name, &wl_data_device_manager_interface, version < 3 ? version : 3);
        printf("ddm-bound\n");
        fflush(stdout);
    }
}

static void registry_global_remove(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_global,
    .global_remove = registry_global_remove,
};

int main(void) {
    g_display = wl_display_connect(NULL);
    if (!g_display) {
        fprintf(stderr, "failed to connect to Wayland display\n");
        return 1;
    }
    g_registry = wl_display_get_registry(g_display);
    wl_registry_add_listener(g_registry, &registry_listener, NULL);
    wl_display_roundtrip(g_display);

    if (!g_drag_manager) {
        fprintf(stderr, "xdg_toplevel_drag_manager_v1 not advertised\n");
        return 2;
    }
    if (!g_ddm) {
        fprintf(stderr, "wl_data_device_manager not advertised\n");
        return 3;
    }

    struct wl_data_source *source = wl_data_device_manager_create_data_source(g_ddm);
    if (!source) {
        fprintf(stderr, "failed to create data source\n");
        return 4;
    }

    struct xdg_toplevel_drag_v1 *drag = xdg_toplevel_drag_manager_v1_get_xdg_toplevel_drag(g_drag_manager, source);
    if (!drag) {
        fprintf(stderr, "failed to get xdg_toplevel_drag\n");
        return 5;
    }

    printf("toplevel-drag-created\n");
    fflush(stdout);
    wl_display_roundtrip(g_display);

    xdg_toplevel_drag_manager_v1_destroy(g_drag_manager);
    wl_data_source_destroy(source);
    wl_registry_destroy(g_registry);
    wl_display_disconnect(g_display);

    printf("probe-success\n");
    fflush(stdout);
    return 0;
}
