/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Register only the exact template reference produced and matched in this task.
 * No biometric payload or Android authentication record is imported. Storage
 * layout follows fprintd v1.94.5 src/file_storage.c. */
#include <fprint.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
int main(void)
{
    if(geteuid()!=0)return 2;
    g_autofree gchar *bytes=NULL;
    g_autoptr(GError) error=NULL;
    gsize length=0;
    if(!g_file_get_contents("/var/lib/aston-fingerprint/native-test-print.bin",&bytes,&length,&error))return 2;
    g_autoptr(FpPrint) print=fp_print_deserialize((guchar *)bytes,length,&error);
    if(!print || strcmp(fp_print_get_driver(print),"uff") || strcmp(fp_print_get_device_id(print),"aston-goodix-g7s-uff"))return 2;
    g_autoptr(GVariant) identifier=NULL;g_object_get(print,"fpi-data",&identifier,NULL);
    if(!identifier || !g_variant_is_of_type(identifier,G_VARIANT_TYPE_UINT32) || g_variant_get_uint32(identifier)!=364578009)return 2;
    fp_print_set_username(print,"logix");fp_print_set_finger(print,FP_FINGER_RIGHT_THUMB);
    g_autofree guchar *data=NULL;gsize size;
    if(!fp_print_serialize(print,&data,&size,&error))return 2;
    const char *directory="/var/lib/fprint/logix/uff/aston-goodix-g7s-uff";
    if(g_mkdir_with_parents(directory,0700))return 2;
    g_autofree gchar *path=g_strdup_printf("%s/%x",directory,FP_FINGER_RIGHT_THUMB);
    int fd=open(path,O_CREAT|O_EXCL|O_WRONLY|O_CLOEXEC|O_NOFOLLOW,0600);
    if(fd<0)return 2;
    ssize_t written=write(fd,data,size);int synced=fsync(fd),closed=close(fd);
    if(written!=(ssize_t)size || synced || closed)return 2;
    printf("Registered existing matched right-thumb template for logix: %s\n",path);
    return 0;
}
