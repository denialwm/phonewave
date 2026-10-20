/* Verify PIN1 once on the primary GW provisioning session over QRTR:
 * UIM Verify PIN (0x0026). The PIN is read from standard input so it never
 * appears in the process list; it is never printed. No retries, no PUK,
 * no PIN change or disable.
 * Message definitions: linux-mobile-broadband/libqmi data/qmi-service-uim.json
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <libqrtr.h>
static unsigned le16(const unsigned char *p) {return p[0]|((unsigned)p[1]<<8);}
static long now_ms(void) {struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec*1000+t.tv_nsec/1000000;}

static const unsigned char *find_tlv(const unsigned char *buf,int len,unsigned type,unsigned *size) {
    for(unsigned off=7;off+3<=(unsigned)len;) {
        unsigned t=buf[off],s=le16(buf+off+1);off+=3;
        if(s>(unsigned)len-off)return NULL;
        if(t==type) {*size=s;return buf+off;}
        off+=s;
    }
    return NULL;
}

int main(int argc,char **argv) {
    if(argc!=3) {fprintf(stderr,"usage: qmi-verify-pin UIM_NODE UIM_PORT < PIN\n");return 2;}
    char *end;unsigned long node=strtoul(argv[1],&end,10);if(*end||node>UINT32_MAX)return 2;
    unsigned long port=strtoul(argv[2],&end,10);if(*end||port>UINT32_MAX)return 2;
    char pin[16];
    if(!fgets(pin,sizeof(pin),stdin)) {fprintf(stderr,"No PIN on standard input\n");return 2;}
    size_t pinlen=strcspn(pin,"\r\n");
    if(pinlen<4||pinlen>8||strspn(pin,"0123456789")!=pinlen) {
        explicit_bzero(pin,sizeof(pin));fprintf(stderr,"PIN must be 4 to 8 digits\n");return 2;
    }

    /* Session: primary GW provisioning (0), empty AID. Info: PIN1 (1), PIN
     * as a string with a one-byte length prefix. */
    unsigned char request[7+3+2+3+2+8];
    unsigned n=7;
    request[n++]=0x01;request[n++]=2;request[n++]=0;request[n++]=0;request[n++]=0;
    request[n++]=0x02;request[n++]=2+pinlen;request[n++]=0;request[n++]=1;request[n++]=pinlen;
    memcpy(request+n,pin,pinlen);n+=pinlen;
    explicit_bzero(pin,sizeof(pin));
    request[0]=0;request[1]=1;request[2]=0;request[3]=0x26;request[4]=0;
    request[5]=(n-7)&0xff;request[6]=(n-7)>>8;

    int fd=qrtr_open(0);if(fd<0) {perror("qrtr_open");return 1;}
    int sent=qrtr_sendto(fd,node,port,request,n);
    explicit_bzero(request,sizeof(request));
    if(sent<0) {perror("send");return 1;}
    unsigned char buf[8192];
    long deadline=now_ms()+10000;
    for(;;) {
        long remaining=deadline-now_ms();if(remaining<=0)break;
        if(qrtr_poll(fd,(unsigned)remaining)<=0)break;
        uint32_t from_node,from_port;int len=qrtr_recvfrom(fd,buf,sizeof(buf),&from_node,&from_port);
        if(len<0) {perror("receive");break;}
        if(from_node!=node||from_port!=port||len<7||buf[0]!=2||le16(buf+1)!=1||le16(buf+3)!=0x26)continue;
        if(le16(buf+5)!=(unsigned)len-7) {fprintf(stderr,"invalid payload size\n");return 1;}
        close(fd);
        unsigned size;const unsigned char *v=find_tlv(buf,len,2,&size);
        if(!v||size!=4) {fprintf(stderr,"No result TLV\n");return 1;}
        printf("UIM Verify PIN: result=%u error=%u\n",le16(v),le16(v+2));
        const unsigned char *r=find_tlv(buf,len,0x10,&size);
        if(r&&size==2)printf("PIN1 retries left=%u PUK1 retries left=%u\n",r[0],r[1]);
        const unsigned char *sw=find_tlv(buf,len,0x13,&size);
        if(sw&&size==2)printf("Card SW1=%02x SW2=%02x\n",sw[0],sw[1]);
        return le16(v)!=0;
    }
    fprintf(stderr,"No response within 10 seconds\n");close(fd);return 1;
}
