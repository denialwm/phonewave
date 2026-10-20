/* Report why the network rejects registration, over QRTR: NAS Get System
 * Info (0x004d) SIM reject state, then NAS Register Indications (0x0003) with
 * Network Reject Information on, then print every Network Reject indication
 * (0x0068) received within SECONDS. Nothing else is changed.
 * Message definitions: linux-mobile-broadband/libqmi data/qmi-service-nas.json
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <libqrtr.h>
static unsigned le16(const unsigned char *p) {return p[0]|((unsigned)p[1]<<8);}
static uint32_t le32(const unsigned char *p) {return le16(p)|((uint32_t)le16(p+2)<<16);}
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

static const char *cause_name(unsigned c) {
    switch(c) {
    case 2: return "IMSI unknown in HLR";
    case 3: return "illegal UE";
    case 5: return "IMEI not accepted";
    case 6: return "illegal ME";
    case 7: return "PS services not allowed";
    case 8: return "PS and non-PS services not allowed";
    case 11: return "PLMN not allowed";
    case 12: return "location area not allowed";
    case 13: return "roaming in location area not allowed";
    case 15: return "no suitable cells in location area";
    default: return "see 3GPP TS 24.008/24.301";
    }
}

static void print_reject(const unsigned char *buf,int len) {
    unsigned size;const unsigned char *v;
    time_t t=time(NULL);char stamp[16];strftime(stamp,sizeof(stamp),"%H:%M:%S",gmtime(&t));
    printf("%s UTC Network Reject:",stamp);
    if((v=find_tlv(buf,len,0x01,&size))&&size==1)printf(" radio=%u",v[0]);
    if((v=find_tlv(buf,len,0x02,&size))&&size==1)printf(" domain=%u",v[0]);
    if((v=find_tlv(buf,len,0x03,&size))&&size==1)printf(" cause=%u (%s)",v[0],cause_name(v[0]));
    if((v=find_tlv(buf,len,0x10,&size))&&size>=4)printf(" PLMN=%u/%u",le16(v),le16(v+2));
    putchar('\n');
}

/* Send one request; print indications that arrive meanwhile. */
static int transact(int fd,uint32_t node,uint32_t port,unsigned txn,unsigned msg,
                    const unsigned char *tlvs,unsigned tlvlen,unsigned char *buf,size_t bufsize) {
    unsigned char request[32];
    request[0]=0;request[1]=txn;request[2]=0;request[3]=msg&0xff;request[4]=msg>>8;
    request[5]=tlvlen;request[6]=0;
    if(tlvlen) memcpy(request+7,tlvs,tlvlen);
    if(qrtr_sendto(fd,node,port,request,7+tlvlen)<0) {perror("send");return -1;}
    long deadline=now_ms()+5000;
    for(;;) {
        long remaining=deadline-now_ms();if(remaining<=0)break;
        if(qrtr_poll(fd,(unsigned)remaining)<=0)break;
        uint32_t n,p;int len=qrtr_recvfrom(fd,buf,bufsize,&n,&p);
        if(len<0) {perror("receive");return -1;}
        if(n!=node||p!=port||len<7||le16(buf+5)!=(unsigned)len-7)continue;
        if(buf[0]==4&&le16(buf+3)==0x68) {print_reject(buf,len);continue;}
        if(buf[0]==2&&le16(buf+1)==txn&&le16(buf+3)==msg)return len;
    }
    fprintf(stderr,"No response to message 0x%04x within 5 seconds\n",msg);
    return -1;
}

int main(int argc,char **argv) {
    if(argc!=4) {fprintf(stderr,"usage: qmi-nas-reject NAS_NODE NAS_PORT SECONDS\n");return 2;}
    char *end;unsigned long node=strtoul(argv[1],&end,10);if(*end||node>UINT32_MAX)return 2;
    unsigned long port=strtoul(argv[2],&end,10);if(*end||port>UINT32_MAX)return 2;
    unsigned long seconds=strtoul(argv[3],&end,10);if(*end||seconds>3600)return 2;
    setvbuf(stdout,NULL,_IOLBF,0);
    unsigned char buf[8192];unsigned size;const unsigned char *v;
    int fd=qrtr_open(0);if(fd<0) {perror("qrtr_open");return 1;}

    int len=transact(fd,node,port,1,0x4d,NULL,0,buf,sizeof(buf));
    if(len<0)return 1;
    if((v=find_tlv(buf,len,0x02,&size))&&size==4)printf("NAS Get System Info: result=%u error=%u\n",le16(v),le16(v+2));
    if((v=find_tlv(buf,len,0x27,&size))&&size==4)
        printf("SIM reject state=%u (0 unavailable, 1 available, 2 CS invalid, 3 PS invalid, 4 CS+PS invalid)\n",le32(v));

    /* Network Reject Information: enable reject indications, keep system info indications. */
    const unsigned char reg[]={0x21,2,0,1,0};
    len=transact(fd,node,port,2,0x03,reg,sizeof(reg),buf,sizeof(buf));
    if(len<0)return 1;
    if((v=find_tlv(buf,len,0x02,&size))&&size==4)printf("NAS Register Indications: result=%u error=%u\n",le16(v),le16(v+2));

    long deadline=now_ms()+(long)seconds*1000;
    for(;;) {
        long remaining=deadline-now_ms();if(remaining<=0)break;
        if(qrtr_poll(fd,(unsigned)remaining)<=0)continue;
        uint32_t n,p;len=qrtr_recvfrom(fd,buf,sizeof(buf),&n,&p);
        if(len<0) {perror("receive");break;}
        if(n!=node||p!=port||len<7||le16(buf+5)!=(unsigned)len-7)continue;
        if(buf[0]==4&&le16(buf+3)==0x68)print_reject(buf,len);
    }
    close(fd);
    return 0;
}
