/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Root-only fixed-operation broker. Biometric buffers remain in signed QTEE. */
#ifndef UFF_RESIDENT_H
#define UFF_RESIDENT_H
#include <sys/socket.h>
#include <sys/un.h>

#define UFF_RESIDENT_SOCKET "/run/aston-fingerprint/worker.sock"
static void resident_notify(const char *message)
{
    const char *path=getenv("NOTIFY_SOCKET");
    struct sockaddr_un address={.sun_family=AF_UNIX};
    if (!path || !*path || strlen(path)>=sizeof(address.sun_path)) return;
    memcpy(address.sun_path,path,strlen(path)+1);
    if (address.sun_path[0]=='@') address.sun_path[0]=0;
    int fd=socket(AF_UNIX,SOCK_DGRAM|SOCK_CLOEXEC,0);
    if (fd>=0) {
        sendto(fd,message,strlen(message),MSG_NOSIGNAL,(struct sockaddr *)&address,
               offsetof(struct sockaddr_un,sun_path)+strlen(path)+1);
        close(fd);
    }
}
static void resident_heartbeat(void) {resident_notify("WATCHDOG=1");}
static int resident_write(int fd,const void *data,size_t size)
{
    const unsigned char *p=data;
    while (size) {
        ssize_t n=write(fd,p,size);
        if (n<0 && errno==EINTR && !worker_stopping) continue;
        if (n<=0) return -1;
        p+=n;size-=n;
    }
    return 0;
}
static int resident_line(int fd,char *line,size_t capacity,int timeout_ms)
{
    size_t used=0;
    while (!worker_stopping && used+1<capacity && timeout_ms>0) {
        struct pollfd pollfd={.fd=fd,.events=POLLIN};
        int interval=timeout_ms>1000?1000:timeout_ms;
        resident_heartbeat();
        int ready=poll(&pollfd,1,interval);
        if (ready<0 && errno==EINTR) continue;
        if (ready<0 || pollfd.revents&(POLLERR|POLLNVAL)) return -1;
        if (!ready) {timeout_ms-=interval;continue;}
        char c;ssize_t n=read(fd,&c,1);
        if (n<0 && errno==EINTR) continue;
        if (n!=1) return -1;
        if (c=='\n') {line[used]=0;return 0;}
        if (c<' ' || c>'~') return -1;
        line[used++]=c;
    }
    return -1;
}
static int resident_standby(struct qcomtee_object *root,struct qcomtee_object *app,
                            uint32_t reset_sequence)
{
    uint32_t standby=1;
    struct uff_fp_state state;
    if (uff_simple(root,app,2019,0x30) ||
        ioctl(uff_power_control,UFF_FP_IOC_SET_STANDBY,&standby) ||
        ioctl(uff_power_control,UFF_FP_IOC_GET_STATE,&state) ||
        state.version!=1 || state.powered!=1 || state.standby!=1 ||
        state.reset_sequence!=reset_sequence) return -1;
    return 0;
}
static int resident_activate(uint32_t reset_sequence)
{
    uint32_t standby=0;struct uff_fp_state state;
    if (ioctl(uff_power_control,UFF_FP_IOC_SET_STANDBY,&standby) ||
        ioctl(uff_power_control,UFF_FP_IOC_GET_STATE,&state) ||
        state.version!=1 || state.powered!=1 || state.standby ||
        state.reset_sequence!=reset_sequence) return -1;
    return 0;
}
static int uff_resident_serve(struct qcomtee_object *root,struct qcomtee_object *app,void *opaque)
{
    (void)opaque;
    struct uff_result initial={0};struct uff_fp_state state;
    uff_heartbeat=resident_heartbeat;
    if (ioctl(uff_power_control,UFF_FP_IOC_ENABLE_POWER) || prepare_sensor(root,app) ||
        uff_service_database(root,app,&initial) ||
        ioctl(uff_power_control,UFF_FP_IOC_GET_STATE,&state) || state.version!=1 ||
        resident_standby(root,app,state.reset_sequence)) return -1;
    struct stat directory;
    if (lstat("/run/aston-fingerprint",&directory) || !S_ISDIR(directory.st_mode) ||
        directory.st_uid!=0 || (directory.st_mode&077)) return -1;
    int listener=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0),ret=-1;
    if (listener<0) return -1;
    struct sockaddr_un address={.sun_family=AF_UNIX};
    memcpy(address.sun_path,UFF_RESIDENT_SOCKET,sizeof(UFF_RESIDENT_SOCKET));
    /* RuntimeDirectory is owned by this unit. Refuse an existing listener;
     * never unlink another process's endpoint to replace its session.
     */
    if (bind(listener,(struct sockaddr *)&address,sizeof(address)) ||
        chmod(UFF_RESIDENT_SOCKET,0600) || listen(listener,4)) goto out;
    resident_notify("READY=1\nSTATUS=Signed sensor sleep retained; accepting native operations");
    fprintf(stderr,"UFF resident ready reset_sequence=%u\n",state.reset_sequence);
    while (!worker_stopping) {
        struct pollfd acceptfd={.fd=listener,.events=POLLIN};
        resident_heartbeat();
        int ready=poll(&acceptfd,1,1000);
        if (ready<0 && errno==EINTR) continue;
        if (ready<0 || acceptfd.revents&(POLLERR|POLLHUP|POLLNVAL)) break;
        if (!ready) continue;
        int client=accept4(listener,NULL,NULL,SOCK_CLOEXEC);
        if (client<0) {if(errno==EINTR)continue;break;}
        struct ucred peer; socklen_t peer_size=sizeof(peer);
        if (getsockopt(client,SOL_SOCKET,SO_PEERCRED,&peer,&peer_size) ||
            peer_size!=sizeof(peer) || peer.uid!=0) {close(client);continue;}
        char request[80];
        if (resident_line(client,request,sizeof(request),2000) || strncmp(request,"UFFC1 ",6)) {
            close(client);continue;
        }
        char *operation=request+6;uint32_t identifier=0;
        if (!strncmp(operation,"delete ",7)) {
            char *end;errno=0;unsigned long value=strtoul(operation+7,&end,10);
            if (errno || *end || !value || value>UINT32_MAX || operation[7]=='0') {
                close(client);continue;
            }
            identifier=(uint32_t)value;operation="delete";
        }
        bool verify=!strcmp(operation,"verify"),enroll=!strcmp(operation,"enroll");
        bool passive=!strcmp(operation,"probe") || !strcmp(operation,"open") || !strcmp(operation,"list");
        if (!verify && !enroll && !passive && strcmp(operation,"delete")) {close(client);continue;}
        FILE *records=fdopen(dup(client),"w");
        if (!records) {close(client);break;}
        setvbuf(records,NULL,_IOLBF,0);
        struct uff_result result={.operation=operation,.records=records,.identifier=identifier};
        int status=0;bool activated=false;
        worker_control_fd=client;
        if (verify) {
            char command[16];
            fprintf(records,"UFW1 standby\n");
            if (fflush(records) || worker_control(command,86400000) || strcmp(command,"activate")) status=-1;
        }
        if (!status && (verify || enroll)) {
            if (resident_activate(state.reset_sequence)) status=-1;
            else activated=true;
        }
        if (!status) {
            if (verify || enroll || !strcmp(operation,"delete")) status=uff_operate(root,app,&result);
            else if (!strcmp(operation,"list")) status=uff_service_database(root,app,&result);
        }
        /* No successful result until cancellation/sleep, IRQ gating and the
         * unchanged physical reset generation have all been confirmed.
         */
        bool parked=!resident_standby(root,app,state.reset_sequence);
        if (!parked) status=-1;
        if (!status && !worker_stopping) {
            if (verify || enroll) fprintf(records,"UFW1 parked\n");
            if (!strcmp(operation,"list"))
                for (uint32_t i=0;i<result.count;i++) fprintf(records,"UFF1 item %u\n",result.ids[i]);
            fprintf(records,"UFF1 result %u %u\n",result.code,result.identifier);
        }
        fprintf(stderr,"UFF resident request=%s activated=%u status=%d parked=%u reset_sequence=%u\n",
                operation,activated,status,parked,state.reset_sequence);
        fclose(records);close(client);worker_control_fd=STDIN_FILENO;
        if (!parked) break;
    }
    ret=worker_stopping?0:-1;
 out:
    close(listener);
    /* RuntimeDirectory cleanup removes the socket after this unit exits. */
    uff_heartbeat=NULL;
    return ret;
}
static int uff_resident_client(int argc,char **argv,FILE *records)
{
    if (device_identity()) return 1;
    struct stat directory,node,self,server;
    if (lstat("/run/aston-fingerprint",&directory) || !S_ISDIR(directory.st_mode) ||
        directory.st_uid!=0 || (directory.st_mode&077) ||
        lstat(UFF_RESIDENT_SOCKET,&node) || !S_ISSOCK(node.st_mode) ||
        node.st_uid!=0 || (node.st_mode&077)) return 1;
    int fd=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0);
    if (fd<0) return 1;
    struct sockaddr_un address={.sun_family=AF_UNIX};
    memcpy(address.sun_path,UFF_RESIDENT_SOCKET,sizeof(UFF_RESIDENT_SOCKET));
    int ret=1;
    if (connect(fd,(struct sockaddr *)&address,sizeof(address))) goto out;
    struct ucred peer;socklen_t peer_size=sizeof(peer);
    char executable[80];
    if (getsockopt(fd,SOL_SOCKET,SO_PEERCRED,&peer,&peer_size) ||
        peer_size!=sizeof(peer) || peer.uid!=0 || peer.pid<=1) goto out;
    snprintf(executable,sizeof(executable),"/proc/%d/exe",peer.pid);
    if (stat("/proc/self/exe",&self) || stat(executable,&server) ||
        self.st_dev!=server.st_dev || self.st_ino!=server.st_ino) goto out;
    char request[80];
    int n=snprintf(request,sizeof(request),"UFFC1 %s%s%s\n",argv[1],argc==4?" ":"",argc==4?argv[3]:"");
    if (n<=0 || n>=(int)sizeof(request) || resident_write(fd,request,n)) goto out;
    bool cancelled=false;size_t total=0;int draining_ms=0;
    while (true) {
        if (worker_stopping && !cancelled) {shutdown(fd,SHUT_WR);cancelled=true;}
        struct pollfd fds[2]={{.fd=fd,.events=POLLIN},
                            {.fd=cancelled?-1:STDIN_FILENO,.events=POLLIN}};
        int ready=poll(fds,2,100);
        if (ready<0 && errno==EINTR) continue;
        if (ready<0 || fds[0].revents&(POLLERR|POLLNVAL)) break;
        if (cancelled && (draining_ms+=100)>15000) break;
        if (fds[0].revents&(POLLIN|POLLHUP)) {
            char buffer[1024];ssize_t bytes=read(fd,buffer,sizeof(buffer));
            if (bytes==0) {ret=cancelled?1:0;break;}
            if (bytes<0) {if(errno==EINTR)continue;break;}
            total+=bytes;
            if (total>16384 || fwrite(buffer,1,bytes,records)!=(size_t)bytes || fflush(records)) break;
        }
        if (fds[1].revents&(POLLIN|POLLHUP)) {
            char buffer[256];ssize_t bytes=read(STDIN_FILENO,buffer,sizeof(buffer));
            if (bytes<0 && errno==EINTR) continue;
            if (bytes<=0) {shutdown(fd,SHUT_WR);cancelled=true;}
            else if (resident_write(fd,buffer,bytes)) break;
        }
    }
 out:
    shutdown(fd,SHUT_RDWR);close(fd);
    return ret;
}
#endif
