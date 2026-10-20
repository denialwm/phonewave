/* Read-only QMI UIM Get Card Status / DMS Get Operating Mode over QRTR.
 * Message definitions: linux-mobile-broadband/libqmi data/qmi-service-{uim,dms}.json
 * No set/reset/PIN/NV commands are implemented.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>
#include <libqrtr.h>
static unsigned le16(const unsigned char *p) { return p[0] | ((unsigned)p[1]<<8); }
static uint32_t le32(const unsigned char *p) { return le16(p) | ((uint32_t)le16(p+2)<<16); }
static long now_ms(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec*1000+t.tv_nsec/1000000; }
int main(int argc, char **argv) {
    if(argc!=4 || (strcmp(argv[1],"uim") && strcmp(argv[1],"dms") && strcmp(argv[1],"slots") && strcmp(argv[1],"nas"))) {
        fprintf(stderr,"usage: qmi-status uim|dms|slots|nas NODE PORT\n"); return 2;
    }
    int nas=!strcmp(argv[1],"nas");
    int uim=!strcmp(argv[1],"uim");
    int slots=!strcmp(argv[1],"slots");
    uint32_t node,port; char *end;
    unsigned long n=strtoul(argv[2],&end,10); if(*end || n>UINT32_MAX) return 2; node=n;
    n=strtoul(argv[3],&end,10); if(*end || n>UINT32_MAX) return 2; port=n;
    unsigned msg=nas?0x24:slots?0x47:(uim?0x2f:0x2d);
    unsigned char request[7]={0,1,0,msg,0,0,0},buf[8192];
    int fd=qrtr_open(0); if(fd<0) {perror("qrtr_open");return 1;}
    if(qrtr_sendto(fd,node,port,request,sizeof(request))<0) {perror("send");return 1;}
    long deadline=now_ms()+5000;
    while(now_ms()<deadline) {
        int ready=qrtr_poll(fd,(unsigned)(deadline-now_ms()));
        if(ready<=0) break;
        uint32_t from_node,from_port;
        int len=qrtr_recvfrom(fd,buf,sizeof(buf),&from_node,&from_port);
        if(len<0) {perror("receive");break;}
        if(from_node!=node || from_port!=port || len<7 || buf[0]!=2 || le16(buf+1)!=1 || le16(buf+3)!=msg) continue;
        if(le16(buf+5)!=(unsigned)len-7) {fprintf(stderr,"invalid payload size\n");return 1;}
        printf("%s response from %u:%u, message 0x%04x\n",argv[1],node,port,msg);
        int result=-1;
        for(unsigned off=7;off<(unsigned)len;) {
            if(off+3>(unsigned)len) return 1;
            unsigned type=buf[off],size=le16(buf+off+1);off+=3;
            if(size>(unsigned)len-off) return 1;
            printf("TLV 0x%02x length %u\n",type,size);
            if(type==2 && size==4) {result=le16(buf+off);printf("QMI result=%d error=%u\n",result,le16(buf+off+2));}
            if(!uim && !slots && !nas && type==1 && size==1) printf("Operating mode=%u\n",buf[off]);
            if(nas && type==1 && size>=5) {
                const unsigned char *v=buf+off;
                if(5u+v[4]>size) return 1;
                printf("Registration=%u CS-attach=%u PS-attach=%u network=%u radios=",v[0],v[1],v[2],v[3]);
                for(unsigned i=0;i<v[4];i++) printf("%u ",v[5+i]);
                puts("");
            }
            if(nas && type==0x12 && size>=4) printf("Serving MCC=%u MNC=%u\n",le16(buf+off),le16(buf+off+2));
            if(uim && type==0x10 && size>=9) {
                unsigned count=buf[off+8],pos=9;
                printf("Card records=%u\n",count);
                for(unsigned i=0;i<count;i++) {
                    if(pos+6>size) return 1;
                    unsigned apps=buf[off+pos+5];
                    printf("Card %u: state=%u error=%u applications=%u\n",i+1,buf[off+pos],buf[off+pos+4],apps);
                    pos+=6;
                    for(unsigned a=0;a<apps;a++) {
                        if(pos+7>size) return 1;
                        unsigned aidlen=buf[off+pos+6];
                        if(pos+14+aidlen>size) return 1;
                        const unsigned char *v=buf+off+pos;
                        unsigned pinpos=7+aidlen;
                        printf("  Application %u: type=%u state=%u personalization=%u PIN1-state=%u PIN1-retries=%u PUK1-retries=%u\n",a+1,v[0],v[1],v[2],v[pinpos+1],v[pinpos+2],v[pinpos+3]);
                        pos+=14+aidlen;
                    }
                }
            }
            if(slots && type==0x10 && size>=1) {
                unsigned count=buf[off],pos=1;
                printf("Physical slots=%u\n",count);
                for(unsigned i=0;i<count;i++) {
                    if(pos+10>size) return 1;
                    const unsigned char *v=buf+off+pos;
                    printf("Physical slot %u: card-state=%u slot-state=%u logical-slot=%u\n",i+1,le32(v),le32(v+4),v[8]);
                    if(pos+10+v[9]>size) return 1;
                    pos+=10+v[9]; /* Do not print ICCID. */
                }
            }
            if(slots && type==0x11 && size>=1) {
                unsigned count=buf[off],pos=1;
                for(unsigned i=0;i<count;i++) {
                    if(pos+6>size) return 1;
                    const unsigned char *v=buf+off+pos;
                    unsigned atrlen=v[5];
                    if(pos+7+atrlen>size) return 1;
                    printf("Physical slot %u: protocol=%u valid-applications=%u eUICC=%u\n",i+1,le32(v),v[4],v[6+atrlen]);
                    pos+=7+atrlen;
                }
            }
            off+=size;
        }
        close(fd);return result==0?0:1;
    }
    fprintf(stderr,"No matching QMI response within 5 seconds\n");close(fd);return 1;
}
