/* SPDX-License-Identifier: LGPL-2.1-or-later */
static int gp_name(const char *path,char name[80])
{
    const char prefix[]="/denial-fingerprint/uff/";
    if (!memchr(path,0,256) || strncmp(path,prefix,sizeof(prefix)-1)) {errno=EACCES;return -1;}
    const char *id=path+sizeof(prefix)-1;
    if (!strcmp(id,"global_finger.db") || !strcmp(id,"global_finger.db.bak") || !strcmp(id,"global_finger.db.bak.bak")) {
        snprintf(name,80,"uff-%s",id);return 0;
    }
    const char *p=id;uint64_t number=0;
    while (*p>='0' && *p<='9') {
        number=number*10+*p++-'0';
        if (number>UINT32_MAX || p-id>10) {errno=EACCES;return -1;}
    }
    if (!number || id[0]=='0' || (*p && strcmp(p,".bak") && strcmp(p,".bak.bak"))) {errno=EACCES;return -1;}
    snprintf(name,80,"uff-template-%s",id);return 0;
}
