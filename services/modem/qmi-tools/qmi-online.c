/* Send only QMI DMS Set Operating Mode ONLINE (0x002e, mode 0).
 * No reset, shutdown, factory-test, PIN, or NV commands.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
#include <libqrtr.h>
static unsigned le16(const unsigned char *p) {return p[0]|((unsigned)p[1]<<8);}
static long now_ms(void) {struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec*1000+t.tv_nsec/1000000;}
int main(int argc,char **argv) {
    if(argc!=3) {fprintf(stderr,"usage: qmi-online DMS_NODE DMS_PORT\n");return 2;}
    char *end;unsigned long node=strtoul(argv[1],&end,10);if(*end||node>UINT32_MAX)return 2;
    unsigned long port=strtoul(argv[2],&end,10);if(*end||port>UINT32_MAX)return 2;
    unsigned char request[]={0,1,0,0x2e,0,4,0,1,1,0,0},buf[8192];
    int fd=qrtr_open(0);if(fd<0){perror("open");return 1;}
    if(qrtr_sendto(fd,node,port,request,sizeof(request))<0){perror("send");return 1;}
    long deadline=now_ms()+10000;
    for(;;) {
        long remaining=deadline-now_ms();if(remaining<=0)break;
        if(qrtr_poll(fd,(unsigned)remaining)<=0)break;
        uint32_t n,p;int len=qrtr_recvfrom(fd,buf,sizeof(buf),&n,&p);
        if(len<0){perror("receive");break;}
        if(n!=node||p!=port||len<7||buf[0]!=2||le16(buf+1)!=1||le16(buf+3)!=0x2e)continue;
        if(le16(buf+5)!=(unsigned)len-7)return 1;
        for(unsigned off=7;off<(unsigned)len;) {
            if(off+3>(unsigned)len)return 1;
            unsigned type=buf[off],size=le16(buf+off+1);off+=3;
            if(size>(unsigned)len-off)return 1;
            if(type==2&&size==4) {
                unsigned result=le16(buf+off),error=le16(buf+off+2);
                printf("DMS Set Operating Mode ONLINE: result=%u error=%u\n",result,error);
                close(fd);return result?1:0;
            }
            off+=size;
        }
    }
    fprintf(stderr,"No matching response within 10 seconds\n");close(fd);return 1;
}
