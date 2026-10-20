/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Small native diagnostic canvas, with no font or Android runtime dependency. */
static const unsigned char diagnostic_font[26][7]={
 {14,17,17,31,17,17,17},{30,17,17,30,17,17,30},{14,17,16,16,16,17,14},
 {30,17,17,17,17,17,30},{31,16,16,30,16,16,31},{31,16,16,30,16,16,16},
 {14,17,16,23,17,17,15},{17,17,17,31,17,17,17},{14,4,4,4,4,4,14},
 {7,2,2,2,18,18,12},{17,18,20,24,20,18,17},{16,16,16,16,16,16,31},
 {17,27,21,21,17,17,17},{17,25,21,19,17,17,17},{14,17,17,17,17,17,14},
 {30,17,17,30,16,16,16},{14,17,17,17,21,18,13},{30,17,17,30,20,18,17},
 {15,16,16,14,1,1,30},{31,4,4,4,4,4,4},{17,17,17,17,17,17,14},
 {17,17,17,17,17,10,4},{17,17,17,21,21,21,10},{17,17,10,4,10,17,17},
 {17,17,10,4,4,4,4},{31,1,2,4,8,16,31}
};
static void diagnostic_text(uint32_t *p, int w, int h, int y, int scale,
                            const char *text, uint32_t color)
{
    int x=(w-(int)strlen(text)*6*scale)/2;
    for (; *text; text++,x+=6*scale) {
        if (*text<'A' || *text>'Z') continue;
        for (int row=0;row<7;row++) for (int col=0;col<5;col++) {
            if (!(diagnostic_font[*text-'A'][row]&(1<<(4-col)))) continue;
            for (int dy=0;dy<scale;dy++) for (int dx=0;dx<scale;dx++) {
                int px=x+col*scale+dx,py=y+row*scale+dy;
                if (px>=0 && px<w && py>=0 && py<h) p[py*w+px]=color;
            }
        }
    }
}
static void diagnostic_canvas(uint32_t *p, int w, int h, const char *phase, int remaining)
{
    const uint32_t black=0xff080b10,white=0xffffffff,cyan=0xff52d9e9;
    for (int i=0;i<w*h;i++) p[i]=black;
    /* Native physical coordinates from Roadstr getSensorProps, 0x6610. */
    double cx=w*610.0/1220,cy=h*2440.0/2712,r=w*102.0/1220;
    for (int y=0;y<h;y++) for (int x=0;x<w;x++) {
        double dx=x+0.5-cx,dy=y+0.5-cy,d=dx*dx+dy*dy;
        if (d<r*r) p[y*w+x]=white;
        else if (d<(r+3)*(r+3)) p[y*w+x]=cyan;
    }
    const char *title="PLACE FINGER",*hint="ON THE WHITE CIRCLE";
    if (!strcmp(phase,"HOLD")) { title="HOLD STILL"; hint="KEEP YOUR FINGER IN PLACE"; }
    if (!strcmp(phase,"LIFT")) { title="LIFT FINGER"; hint="THEN SHIFT SLIGHTLY"; }
    if (!strcmp(phase,"DONE")) { title="COMPLETE"; hint="YOU CAN LIFT YOUR FINGER"; }
    int scale=w/240;
    if (scale<1) scale=1;
    diagnostic_text(p,w,h,h/3,scale*2,title,white);
    diagnostic_text(p,w,h,h/3+35*scale,scale,hint,cyan);
    if (remaining<0) remaining=0;
    if (remaining>20) remaining=20;
    int cell=w/28,start=(w-20*cell)/2;
    for (int i=0;i<20;i++) for (int y=h/2;y<h/2+8*scale;y++)
        for (int x=start+i*cell;x<start+(i+1)*cell-3;x++)
            p[y*w+x]=i<20-remaining ? cyan : 0xff303846;
}
