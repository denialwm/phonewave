/* SPDX-License-Identifier: LGPL-2.1-or-later */
#ifndef GOODIX_RESIDENT_H
#define GOODIX_RESIDENT_H
/* Cold-session broker: each request unloads its app and powers off. The
 * NX809J control ABI has no retained-power standby operation. */
#define GOODIX_SOCKET "/run/nx809j-fingerprint/worker.sock"
#define GOODIX_RUNTIME "/run/nx809j-fingerprint"

static void resident_notify(const char *message)
{
    const char *path=getenv("NOTIFY_SOCKET");
    struct sockaddr_un address={.sun_family=AF_UNIX};
    if (!path || !*path || strlen(path)>=sizeof(address.sun_path)) return;
    size_t length=strlen(path);
    memcpy(address.sun_path,path,length+1);
    if (path[0]=='@') address.sun_path[0]=0;
    int fd=socket(AF_UNIX,SOCK_DGRAM|SOCK_CLOEXEC,0);
    if (fd>=0) {
        sendto(fd,message,strlen(message),MSG_NOSIGNAL,(struct sockaddr *)&address,
               offsetof(struct sockaddr_un,sun_path)+length+1);
        close(fd);
    }
}

static int resident_write(int fd,const void *data,size_t size)
{
    const uint8_t *bytes=data;
    while (size && !worker_stopping) {
        ssize_t n=send(fd,bytes,size,MSG_NOSIGNAL);
        if (n<0 && errno==EINTR) continue;
        if (n<=0) return -1;
        bytes+=n;size-=(size_t)n;
    }
    return size?-1:0;
}

static int resident_request(char *line,const char **operation,uint32_t *identifier)
{
    if (strncmp(line,"UFFC1 ",6)) return -1;
    *operation=line+6;*identifier=0;
    if (!strncmp(*operation,"delete ",7)) {
        const char *number=*operation+7;char *end;errno=0;
        unsigned long value=strtoul(number,&end,10);
        if (errno || *end || !value || value>UINT32_MAX || number[0]=='0') return -1;
        *operation="delete";*identifier=(uint32_t)value;return 0;
    }
    static const char *allowed[]={"probe","open","list","check-enroll","enroll","verify"};
    for (size_t i=0;i<sizeof(allowed)/sizeof(*allowed);i++) if (!strcmp(*operation,allowed[i])) return 0;
    return -1;
}

static int resident_serve(const char *firmware)
{
    struct stat directory,owned_node;
    if (lstat(GOODIX_RUNTIME,&directory) || !S_ISDIR(directory.st_mode) || directory.st_uid || (directory.st_mode&077)) return 1;
    int listener=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0),ret=1;
    bool bound=false;
    if (listener<0) return 1;
    struct sockaddr_un address={.sun_family=AF_UNIX};
    memcpy(address.sun_path,GOODIX_SOCKET,sizeof(GOODIX_SOCKET));
    if (bind(listener,(struct sockaddr *)&address,sizeof(address))) goto done;
    if (lstat(GOODIX_SOCKET,&owned_node)) goto done;
    bound=true;
    if (chmod(GOODIX_SOCKET,0600) || listen(listener,4)) goto done;
    resident_active=true;
    resident_notify("READY=1\nSTATUS=Accepting native Goodix requests; sensor powered off between sessions");
    while (!worker_stopping) {
        resident_notify("WATCHDOG=1");
        struct pollfd p={.fd=listener,.events=POLLIN};
        int ready=poll(&p,1,1000);
        if (ready<0 && errno==EINTR) continue;
        if (ready<0 || p.revents&(POLLERR|POLLHUP|POLLNVAL)) goto done;
        if (!ready) continue;
        int client=accept4(listener,NULL,NULL,SOCK_CLOEXEC);
        if (client<0) {if(errno==EINTR)continue;goto done;}
        struct ucred peer;socklen_t size=sizeof(peer);
        if (getsockopt(client,SOL_SOCKET,SO_PEERCRED,&peer,&size) || size!=sizeof(peer) || peer.uid) {close(client);continue;}
        worker_control_fd=client;
        char line[80];const char *operation;uint32_t identifier;
        if (worker_control(NULL,line,sizeof(line),2000) || resident_request(line,&operation,&identifier)) {
            close(client);worker_control_fd=STDIN_FILENO;continue;
        }
        FILE *records=fdopen(dup(client),"w");
        if (!records) {close(client);goto done;}
        setvbuf(records,NULL,_IOLBF,0);
        if (!strcmp(operation,"probe") || !strcmp(operation,"open")) {
            if (!worker_passive()) fprintf(records,"UFF1 result 0 0\n");
        } else (void)worker_run(firmware,operation,identifier,records);
        fclose(records);close(client);worker_control_fd=STDIN_FILENO;
    }
    ret=0;
 done:
    resident_active=false;
    close(listener);
    if (bound) {
        struct stat current;
        if (!lstat(GOODIX_SOCKET,&current) && current.st_dev==owned_node.st_dev && current.st_ino==owned_node.st_ino)
            unlink(GOODIX_SOCKET);
    }
    return ret;
}

static int resident_client(int argc,char **argv,FILE *records)
{
    /* Descriptor passing is deliberately not implicit. For resident enroll,
     * send hat\n and the 69 opaque bytes over the private control stream. */
    if (getenv("GOODIX_HAT_FD")) {
        fputs("Resident enrollment uses the private hat control message\n",stderr);return 2;
    }
    struct stat directory,node,self,server;
    if (lstat(GOODIX_RUNTIME,&directory) || !S_ISDIR(directory.st_mode) || directory.st_uid || (directory.st_mode&077) ||
        lstat(GOODIX_SOCKET,&node) || !S_ISSOCK(node.st_mode) || node.st_uid || (node.st_mode&077)) return 1;
    int fd=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0),ret=1;
    if (fd<0) return 1;
    struct sockaddr_un address={.sun_family=AF_UNIX};
    memcpy(address.sun_path,GOODIX_SOCKET,sizeof(GOODIX_SOCKET));
    if (connect(fd,(struct sockaddr *)&address,sizeof(address))) goto done;
    struct ucred peer;socklen_t size=sizeof(peer);char executable[80];
    if (getsockopt(fd,SOL_SOCKET,SO_PEERCRED,&peer,&size) || size!=sizeof(peer) || peer.uid || peer.pid<=1) goto done;
    snprintf(executable,sizeof(executable),"/proc/%d/exe",peer.pid);
    if (stat("/proc/self/exe",&self) || stat(executable,&server) || self.st_dev!=server.st_dev || self.st_ino!=server.st_ino) goto done;
    char request[80];
    int n=snprintf(request,sizeof(request),"UFFC1 %s%s%s\n",argv[1],argc==4?" ":"",argc==4?argv[3]:"");
    if (n<=0 || n>=(int)sizeof(request) || resident_write(fd,request,n)) goto done;
    char received[16385]={0};size_t total=0;bool cancelled=false;
    bool interactive=!strcmp(argv[1],"verify") || !strcmp(argv[1],"enroll") || !strcmp(argv[1],"check-enroll");
    int64_t deadline=INT64_MAX;
    while (true) {
        if (worker_stopping && !cancelled) {shutdown(fd,SHUT_WR);cancelled=true;deadline=milliseconds()+15000;}
        if (milliseconds()>=deadline) break;
        struct pollfd p[2]={{.fd=fd,.events=POLLIN},{.fd=cancelled || !interactive?-1:STDIN_FILENO,.events=POLLIN}};
        int ready=poll(p,2,100);
        if (ready<0 && errno==EINTR) continue;
        if (ready<0 || p[0].revents&(POLLERR|POLLNVAL)) break;
        if (p[0].revents&(POLLIN|POLLHUP)) {
            char buffer[1024];ssize_t count=read(fd,buffer,sizeof(buffer));
            if (!count) {
                if (!cancelled && (!strncmp(received,"UFF1 result ",12) || strstr(received,"\nUFF1 result "))) ret=0;
                break;
            }
            if (count<0) {if(errno==EINTR)continue;break;}
            if ((size_t)count>sizeof(received)-1-total) break;
            memcpy(received+total,buffer,count);total+=(size_t)count;
            if (fwrite(buffer,1,count,records)!=(size_t)count || fflush(records)) break;
        }
        if (p[1].revents&(POLLIN|POLLHUP)) {
            uint8_t buffer[256];ssize_t count=read(STDIN_FILENO,buffer,sizeof(buffer));
            if (count<0 && errno==EINTR) continue;
            if (count<=0) {shutdown(fd,SHUT_WR);cancelled=true;deadline=milliseconds()+15000;}
            else {
                int failed=resident_write(fd,buffer,count);
                explicit_bzero(buffer,sizeof(buffer));
                if (failed) break;
            }
        }
    }
 done:
    shutdown(fd,SHUT_RDWR);close(fd);return ret;
}
#endif
