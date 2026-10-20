/* Activate the primary GW provisioning session on one UIM slot over QRTR:
 * UIM Get Card Status (0x002f), then UIM Change Provisioning Session (0x0038)
 * with the first USIM application on that slot. No PIN, reset or NV commands.
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

/* Send one request and return the length of its response in buf, or -1. */
static int transact(int fd,uint32_t node,uint32_t port,unsigned txn,unsigned msg,
                    const unsigned char *tlvs,unsigned tlvlen,unsigned char *buf,size_t bufsize) {
    unsigned char request[64];
    if(7+tlvlen>sizeof(request)) return -1;
    request[0]=0;request[1]=txn&0xff;request[2]=txn>>8;
    request[3]=msg&0xff;request[4]=msg>>8;request[5]=tlvlen&0xff;request[6]=tlvlen>>8;
    if(tlvlen) memcpy(request+7,tlvs,tlvlen);
    if(qrtr_sendto(fd,node,port,request,7+tlvlen)<0) {perror("send");return -1;}
    long deadline=now_ms()+10000;
    for(;;) {
        long remaining=deadline-now_ms();if(remaining<=0)break;
        if(qrtr_poll(fd,(unsigned)remaining)<=0)break;
        uint32_t n,p;int len=qrtr_recvfrom(fd,buf,bufsize,&n,&p);
        if(len<0) {perror("receive");return -1;}
        if(n!=node||p!=port||len<7||buf[0]!=2||le16(buf+1)!=txn||le16(buf+3)!=msg)continue;
        if(le16(buf+5)!=(unsigned)len-7) {fprintf(stderr,"invalid payload size\n");return -1;}
        return len;
    }
    fprintf(stderr,"No response to message 0x%04x within 10 seconds\n",msg);
    return -1;
}

static const unsigned char *find_tlv(const unsigned char *buf,int len,unsigned type,unsigned *size) {
    for(unsigned off=7;off+3<=(unsigned)len;) {
        unsigned t=buf[off],s=le16(buf+off+1);off+=3;
        if(s>(unsigned)len-off)return NULL;
        if(t==type) {*size=s;return buf+off;}
        off+=s;
    }
    return NULL;
}

static int check_result(const char *what,const unsigned char *buf,int len) {
    unsigned size;const unsigned char *v=find_tlv(buf,len,2,&size);
    if(!v||size!=4) {fprintf(stderr,"%s: no result TLV\n",what);return 1;}
    printf("%s: result=%u error=%u\n",what,le16(v),le16(v+2));
    return le16(v)!=0;
}

int main(int argc,char **argv) {
    if(argc!=4) {fprintf(stderr,"usage: qmi-provision UIM_NODE UIM_PORT SLOT\n");return 2;}
    char *end;unsigned long node=strtoul(argv[1],&end,10);if(*end||node>UINT32_MAX)return 2;
    unsigned long port=strtoul(argv[2],&end,10);if(*end||port>UINT32_MAX)return 2;
    unsigned long slot=strtoul(argv[3],&end,10);if(*end||slot<1||slot>5)return 2;
    unsigned char buf[8192];
    int fd=qrtr_open(0);if(fd<0) {perror("qrtr_open");return 1;}

    int len=transact(fd,node,port,1,0x2f,NULL,0,buf,sizeof(buf));
    if(len<0||check_result("UIM Get Card Status",buf,len))return 1;
    unsigned size;const unsigned char *v=find_tlv(buf,len,0x10,&size);
    if(!v||size<9)return 1;
    const unsigned char *aid=NULL;unsigned aidlen=0,cards=v[8],pos=9;
    for(unsigned card=1;card<=cards;card++) {
        if(pos+6>size)return 1;
        unsigned apps=v[pos+5];pos+=6;
        for(unsigned a=0;a<apps;a++) {
            if(pos+7>size)return 1;
            unsigned l=v[pos+6];
            if(pos+14+l>size)return 1;
            if(card==slot&&!aid&&v[pos]==2) {aid=v+pos+7;aidlen=l;}
            pos+=14+l;
        }
    }
    if(!aid||aidlen>32) {fprintf(stderr,"No USIM application on slot %lu\n",slot);return 1;}
    printf("Slot %lu USIM application identifier: %u bytes\n",slot,aidlen);

    /* Session Change: primary GW provisioning (0), activate. Application
     * Information: slot, AID with a one-byte length prefix. */
    unsigned char tlvs[3+2+3+2+32];
    unsigned n=0;
    tlvs[n++]=0x01;tlvs[n++]=2;tlvs[n++]=0;tlvs[n++]=0;tlvs[n++]=1;
    tlvs[n++]=0x10;tlvs[n++]=2+aidlen;tlvs[n++]=0;tlvs[n++]=slot;tlvs[n++]=aidlen;
    memcpy(tlvs+n,aid,aidlen);n+=aidlen;
    len=transact(fd,node,port,2,0x38,tlvs,n,buf,sizeof(buf));
    close(fd);
    if(len<0)return 1;
    return check_result("UIM Change Provisioning Session",buf,len);
}
