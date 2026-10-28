/* SPDX-License-Identifier: LGPL-2.1-or-later */
#define _GNU_SOURCE
#include "goodix-worker-ops.h"
#include <signal.h>
#include <pthread.h>
#include <assert.h>
static volatile sig_atomic_t worker_stopping;
#include "goodix-authorization.h"
struct fixture { int fd; int status; unsigned bad_magic; };
static void *broker(void *opaque)
{
    struct fixture *f=opaque; uint8_t request[24]={0},reply[16]={'N','X','E','R','1'},token[69];
    assert(read(f->fd,request,24)==24);
    assert(!memcmp(request,"NXEA1\0\0\0",8));
    assert(request[8]==0x34 && request[9]==0x12);
    uint32_t uid;memcpy(&uid,request+16,4);assert(uid==GOODIX_NATIVE_GROUP);
    memcpy(reply+8,&f->status,4);
    if (f->bad_magic) reply[0]='X';
    assert(write(f->fd,reply,16)==16);
    if (!f->status && !f->bad_magic) {
        memset(token,0xab,sizeof(token));assert(write(f->fd,token,69)==69);
    }
    close(f->fd);return NULL;
}
int main(void)
{
    for (unsigned mode=0;mode<3;mode++) {
        int fds[2];assert(!socketpair(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0,fds));
        struct fixture f={.fd=fds[1],.status=mode==1?-EAGAIN:0,.bad_magic=mode==2};
        pthread_t thread;assert(!pthread_create(&thread,NULL,broker,&f));
        uint8_t token[69];memset(token,0xcc,sizeof(token));
        int ret=authorization_exchange(fds[0],0x1234,token);
        assert(ret==(mode==1?-EAGAIN:mode==2?-EPROTO:0));
        for (unsigned i=0;i<69;i++) assert(token[i]==(mode==0?0xab:0));
        close(fds[0]);assert(!pthread_join(thread,NULL));
    }
    worker_stopping=1;int fds[2];assert(!socketpair(AF_UNIX,SOCK_STREAM,0,fds));
    uint8_t token[69];assert(authorization_exchange(fds[0],1,token)==-ECANCELED);
    close(fds[0]);close(fds[1]);worker_stopping=0;
    (void)authorization_available;(void)worker_authorize;
    puts("PASS: challenge broker framing, retry failure, malformed reply and cancellation");
    return 0;
}
