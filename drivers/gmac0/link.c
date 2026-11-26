/* SPDX-License-Identifier: BSD-2-Clause */
#include "link.h"
#include "link-config.h"
#include "mdio.h"
uint32_t link_mac_version,link_phy_id,link_clock_hz;
static unsigned controlled;
void link_stop(void) {if (controlled) hw_write(3,0x434,0x00800080);}
static void wait_us(unsigned duration) {
    uint64_t start=hw_us();
    while (hw_us()-start<duration) hw_progress();
}
static uint32_t mdio_r(void *context,unsigned offset) {(void)context;return hw_read(0,offset);}
static void mdio_w(void *context,unsigned offset,uint32_t value) {(void)context;hw_write(0,offset,value);}
static uint64_t mdio_time(void *context) {(void)context;return hw_us();}
static struct mdio_io mdio={0,mdio_r,mdio_w,mdio_time};
static int read_phy(unsigned reg,uint16_t *value) {int result=mdio_read(&mdio,PHY_ADDRESS,reg,1,value);hw_progress();return result;}
static int write_phy(unsigned reg,uint16_t value) {int result=mdio_write(&mdio,PHY_ADDRESS,reg,1,value);hw_progress();return result;}
int link_allowed(unsigned region,unsigned offset,uint32_t value) {
    if (region==0) {
        if (offset==0x200) return (value&~((31u<<21)|(31u<<16)|(15u<<8)|13u))==0;
        if (offset==0x204) return value<=0xffff;
        if (offset==0) return !(value&3u);
        if (offset==0x1104 || offset==0x1108) return value==0;
        return 0;
    }
    for (unsigned i=0;i<sizeof(setup)/sizeof(*setup);i++) {
        const struct setup_write *s=&setup[i];
        if (region==s->region && offset==s->offset && value==((s->mask<<16)|s->value)) return 1;
    }
    if (region==3) {
        if (offset==0x300+14*4) return value==0x05000000;
        if (offset==0x300+15*4) return value==0x10e00000;
        if (offset==0x300+35*4) return value==0x04000000;
        if (offset==0x17c) return value==0x03370000 || value==0x00300000 || value==0x00300020 || value==0x00300030;
        if (offset==0x434) return value==0x00800080 || value==0x00800000;
    }
    if (region==4) return offset==0x184 && value==0x02000000;
    if (region==5) return (offset==4 && (value==0x00200000 || value==0x00200020)) || (offset==12 && value==0x00200020);
    return 0;
}
static int stopped(void) {
    if ((hw_read(0,0)&3) || (hw_read(0,0x1104)&1) || (hw_read(0,0x1108)&1)) return 0;
    for (unsigned offset=0x1110;offset<=0x111c;offset+=4) if (hw_read(0,offset)) return 0;
    return 1;
}
int link_init(void) {
    controlled=0;
    if ((hw_read(2,0x148)&0x22)!=0x20 || (hw_read(2,0x140)&0x52)!=0x50 ||
        (hw_read(2,0x144)&0x52)!=0x02) return 9;
    unsigned con0=hw_read(3,0x60),con1=hw_read(3,0x64),con2=hw_read(3,0x68);
    unsigned ref=con1&63,post1=(con0>>12)&7,post2=(con1>>6)&7;
    if (!(hw_read(3,0xc0)&(1u<<4)) || !ref || !post1 || !post2) return 1;
    uint64_t scaled=((uint64_t)(con0&4095)<<24)+((con1&(1u<<12))?0:con2&0xffffff);
    uint64_t hz=(24000000ull*scaled/ref/post1/post2)>>24;
    hz/=1+(hw_read(3,0x240)&31);
    if (hz!=125000000) return 1;
    link_clock_hz=(uint32_t)hz;
    controlled=1;
    hw_write(3,0x434,0x00800080);
    if (!(hw_read(3,0x434)&0x80)) return 8;
    hw_write(3,0x38c,0x04000000);
    hw_write(3,0x338,0x05000000);
    hw_write(3,0x33c,0x10e00000);
    hw_write(3,0x17c,0x03370000);
    hw_write(4,0x184,0x02000000);
    uint32_t gpio_version=hw_read(5,0x78);
    if (!gpio_version || gpio_version==0xffffffffu) return 8;
    hw_write(5,4,0x00200020);hw_write(5,12,0x00200020);
    for (unsigned i=0;i<sizeof(setup)/sizeof(*setup);i++) hw_write(setup[i].region,setup[i].offset,(setup[i].mask<<16)|setup[i].value);
    for (unsigned i=0;i<sizeof(setup)/sizeof(*setup);i++) if ((hw_read(setup[i].region,setup[i].offset)&setup[i].mask)!=setup[i].value) return 8;
    if ((hw_read(3,0x38c)&0x400) || (hw_read(3,0x338)&0x500) ||
        (hw_read(3,0x33c)&0x10e0) || (hw_read(3,0x17c)&0x337) || (hw_read(4,0x184)&0x200)) return 8;
    wait_us(1000);hw_write(3,0x434,0x00800000);wait_us(1000);
    if (hw_read(3,0x434)&0x80) return 8;
    link_mac_version=hw_read(0,0x110);
    unsigned revision=link_mac_version&255;
    if (!(revision==0x40 || revision==0x41 || revision==0x50 || revision==0x51 || revision==0x52)) return 2;
    hw_write(0,0,hw_read(0,0)&~3u);hw_write(0,0x1104,0);hw_write(0,0x1108,0);
    if (!stopped()) return 3;
    hw_write(5,4,0x00200000);wait_us(15000);hw_write(5,4,0x00200020);wait_us(50000);
    uint16_t hi,lo,value;
    if (write_phy(31,0) || read_phy(2,&hi) || read_phy(3,&lo)) return 4;
    link_phy_id=((uint32_t)hi<<16)|lo;
    if (link_phy_id!=EXPECT_PHY_ID) return 5;
    if (write_phy(0,0x8000)) return 4;
    uint64_t start=hw_us();
    do {if (read_phy(0,&value)) return 4;if (hw_us()-start>=500000) return 6;hw_progress();} while (value&0x8000);
    if (write_phy(31,0xd08) || read_phy(17,&value) || write_phy(17,value&~0x100u) || read_phy(21,&value) || write_phy(21,value&~8u) || write_phy(31,0)) return 4;
    if (write_phy(13,7) || write_phy(14,0x3c) || write_phy(13,0x4007) || write_phy(14,0) || write_phy(13,0)) return 4;
    if (write_phy(4,0x01e1) || write_phy(9,0x0200) || write_phy(0,0x1200)) return 4;
    return stopped()?0:3;
}
int link_poll(unsigned *speed,unsigned *duplex) {
    uint16_t status,bmsr,hi,lo;
    *speed=*duplex=0;
    if (!stopped()) return -3;
    if (write_phy(31,0) || read_phy(2,&hi) || read_phy(3,&lo) || read_phy(1,&bmsr) || read_phy(1,&bmsr)) return -4;
    link_phy_id=((uint32_t)hi<<16)|lo;
    if (link_phy_id!=EXPECT_PHY_ID || !bmsr || bmsr==0xffff) return -5;
    if (!(bmsr&4) || !(bmsr&32)) return 0;
    if (write_phy(31,0xa43) || read_phy(26,&status) || write_phy(31,0)) return -4;
    if (!(status&4)) return 0;
    unsigned code=(status>>4)&3;
    if (code==3) return -7;
    *speed=code==2?1000:code==1?100:10;*duplex=!!(status&8);
    if (*speed==1000 && !*duplex) return -7;
    hw_write(3,0x17c,0x00300000|(*speed==1000?0:*speed==100?0x30:0x20));
    uint32_t config=hw_read(0,0)&~((1u<<13)|(1u<<14)|(1u<<15)|3u);
    config|=(*duplex?1u<<13:0)|(*speed!=1000?1u<<15:0)|(*speed==100?1u<<14:0);
    hw_write(0,0,config);
    return 1;
}
