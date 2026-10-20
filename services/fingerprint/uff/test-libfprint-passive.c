/* SPDX-License-Identifier: LGPL-2.1-or-later */
#include <fprint.h>
#include <stdio.h>
#include <string.h>
int main(void)
{
    g_autoptr(FpContext) context=fp_context_new();
    fp_context_enumerate(context);
    GPtrArray *devices=fp_context_get_devices(context);
    if (devices->len!=1) {fprintf(stderr,"Expected one native sensor, got %u\n",devices->len);return 2;}
    FpDevice *device=g_ptr_array_index(devices,0);
    if (strcmp(fp_device_get_driver(device),"uff")) return 2;
    g_autoptr(GError) error=NULL;
    g_autoptr(GPtrArray) prints=NULL;
    if (!fp_device_open_sync(device,NULL,&error)) goto failed;
    prints=fp_device_list_prints_sync(device,NULL,&error);
    if (!prints) goto failed;
    if (prints->len) {fprintf(stderr,"Expected empty native test database\n");return 2;}
    if (!fp_device_close_sync(device,NULL,&error)) goto failed;
    puts("Native libfprint discovery, open, empty list, close: PASS (no optical operation)");
    return 0;
failed:
    fprintf(stderr,"libfprint: %s\n",error?error->message:"unknown failure");
    return 2;
}
