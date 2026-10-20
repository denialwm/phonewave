/* Check the identities the modem reports over QRTR: DMS Get IDs (0x0025).
 * Prints only the IMEI length, its type allocation code (the first eight
 * digits, which identify the model) and whether its check digit is valid;
 * never the serial part. ESN and MEID are reported as present or absent.
 * Message definitions: linux-mobile-broadband/libqmi data/qmi-service-dms.json
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
#include <libqrtr.h>
static unsigned le16(const unsigned char *p) {return p[0]|((unsigned)p[1]<<8);}
static long now_ms(void) {struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec*1000+t.tv_nsec/1000000;}

static int luhn_valid(const unsigned char *d,unsigned n) {
    unsigned sum=0;
    for(unsigned i=0;i<n;i++) {
        if(d[i]<'0'||d[i]>'9')return 0;
        unsigned v=d[i]-'0';
        if((n-1-i)%2) {v*=2;if(v>9)v-=9;}
        sum+=v;
    }
    return sum%10==0;
}

int main(int argc,char **argv) {
    if(argc!=3) {fprintf(stderr,"usage: qmi-ids DMS_NODE DMS_PORT\n");return 2;}
    char *end;unsigned long node=strtoul(argv[1],&end,10);if(*end||node>UINT32_MAX)return 2;
    unsigned long port=strtoul(argv[2],&end,10);if(*end||port>UINT32_MAX)return 2;
    unsigned char request[7]={0,1,0,0x25,0,0,0},buf[8192];
    int fd=qrtr_open(0);if(fd<0) {perror("qrtr_open");return 1;}
    if(qrtr_sendto(fd,node,port,request,sizeof(request))<0) {perror("send");return 1;}
    long deadline=now_ms()+5000;
    for(;;) {
        long remaining=deadline-now_ms();if(remaining<=0)break;
        if(qrtr_poll(fd,(unsigned)remaining)<=0)break;
        uint32_t n,p;int len=qrtr_recvfrom(fd,buf,sizeof(buf),&n,&p);
        if(len<0) {perror("receive");break;}
        if(n!=node||p!=port||len<7||buf[0]!=2||le16(buf+1)!=1||le16(buf+3)!=0x25)continue;
        if(le16(buf+5)!=(unsigned)len-7) {fprintf(stderr,"invalid payload size\n");return 1;}
        close(fd);
        int result=-1;
        for(unsigned off=7;off<(unsigned)len;) {
            if(off+3>(unsigned)len)return 1;
            unsigned type=buf[off],size=le16(buf+off+1);off+=3;
            if(size>(unsigned)len-off)return 1;
            const unsigned char *v=buf+off;
            if(type==0x02&&size==4) {result=le16(v);printf("DMS Get IDs: result=%u error=%u\n",le16(v),le16(v+2));}
            if(type==0x10)printf("ESN present, %u characters\n",size);
            if(type==0x12)printf("MEID present, %u characters\n",size);
            if(type==0x11) {
                unsigned zeros=0;
                for(unsigned i=0;i<size;i++)zeros+=v[i]=='0';
                printf("IMEI %u characters, TAC %.*s, check digit %s%s\n",size,size<8?(int)size:8,(const char*)v,
                       size==15&&luhn_valid(v,size)?"valid":"INVALID",zeros==size?", all zeros":"");
            }
            off+=size;
        }
        return result!=0;
    }
    fprintf(stderr,"No matching QMI response within 5 seconds\n");close(fd);return 1;
}
