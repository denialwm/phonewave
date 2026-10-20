/* SPDX-License-Identifier: LGPL-2.1-or-later */
#include <fprint.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
static void progress(FpDevice *d,gint n,FpPrint *p,gpointer user,GError *error)
{
    (void)d;(void)p;(void)user;
    printf("LIBFPRINT progress %d%s\n",n,error?" retry":"");fflush(stdout);
}
int main(int argc,char **argv)
{
    if (getuid()!=0 || argc!=2 || (strcmp(argv[1],"enroll") && strcmp(argv[1],"verify") && strcmp(argv[1],"recover"))) return 2;
    const char *path="/var/lib/aston-fingerprint/native-test-print.bin";
    g_autoptr(FpContext) context=fp_context_new();fp_context_enumerate(context);
    GPtrArray *devices=fp_context_get_devices(context);if(devices->len!=1)return 2;
    FpDevice *device=g_ptr_array_index(devices,0);if(strcmp(fp_device_get_driver(device),"uff"))return 2;
    g_autoptr(GError) error=NULL;
    g_autoptr(FpPrint) print=NULL;
    g_autofree guchar *data=NULL;
    gsize size=0;int ret=2;
    if(!fp_device_open_sync(device,NULL,&error))goto done;
    if(strcmp(argv[1],"verify")) {
        if(!access(path,F_OK)) {fputs("Existing test print must be preserved\n",stderr);goto done;}
        if(!strcmp(argv[1],"recover")) {
            g_autoptr(GPtrArray) prints=fp_device_list_prints_sync(device,NULL,&error);
            if(!prints || prints->len!=1)goto done;
            print=g_object_ref(g_ptr_array_index(prints,0));
            if(g_object_is_floating(print))g_object_ref_sink(print);
            g_autoptr(GVariant) identifier=NULL;
            g_object_get(print,"fpi-data",&identifier,NULL);
            if(!identifier || !g_variant_is_of_type(identifier,G_VARIANT_TYPE_UINT32) || g_variant_get_uint32(identifier)!=364578009)goto done;
            fp_print_set_username(print,"logix");
        } else {
            g_autoptr(FpPrint) seed=g_object_ref_sink(fp_print_new(device));
            fp_print_set_username(seed,"logix");
            print=fp_device_enroll_sync(device,seed,NULL,progress,NULL,&error);
        }
        if(!print || !fp_print_serialize(print,&data,&size,&error))goto done;
        int fd=open(path,O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC|O_NOFOLLOW,0600);
        if(fd<0)goto done;
        ssize_t written=write(fd,data,size);int sync=fsync(fd),closed=close(fd);
        if(written!=(ssize_t)size || sync || closed)goto done;
        puts("LIBFPRINT enrolled and serialized native template reference (phone only)");ret=0;
    } else {
        int fd=open(path,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);if(fd<0)goto done;
        unsigned char buffer[4096];ssize_t length=read(fd,buffer,sizeof(buffer));close(fd);
        if(length<=0 || length==(ssize_t)sizeof(buffer))goto done;
        print=fp_print_deserialize(buffer,length,&error);explicit_bzero(buffer,sizeof(buffer));
        if(!print || !fp_print_compatible(print,device))goto done;
        gboolean match=FALSE;
        if(!fp_device_verify_sync(device,print,NULL,NULL,NULL,&match,NULL,&error))goto done;
        printf("LIBFPRINT match %d\n",match);ret=match?0:1;
    }
 done:
    if(error)fprintf(stderr,"LIBFPRINT error: %s\n",error->message);
    if(fp_device_is_open(device)) {
        g_clear_error(&error);
        if(!fp_device_close_sync(device,NULL,&error)) {fprintf(stderr,"Close failed: %s\n",error->message);ret=2;}
    }
    return ret;
}
