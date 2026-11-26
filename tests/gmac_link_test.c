#include <assert.h>
#include <stdint.h>
#include <string.h>
#include "link.h"
#include "link-config.h"
static uint32_t regs[6][2048];
static uint16_t phy[4096][32];
static unsigned page,busy,writes,progress;
static uint64_t time_us;
uint64_t hw_us(void) {return ++time_us;}
void hw_progress(void) {progress++;}
uint32_t hw_read(unsigned region,unsigned offset) {
    if (!region && offset==0x200 && busy) return 1;
    return regs[region][offset/4];
}
void hw_write(unsigned region,unsigned offset,uint32_t value) {
    assert(link_allowed(region,offset,value));
    if (!writes) assert(region==3 && offset==0x434 && value==0x00800080);
    writes++;
    if (region) {unsigned mask=value>>16;regs[region][offset/4]=(regs[region][offset/4]&~mask)|(value&mask);return;}
    regs[region][offset/4]=value;
    if (offset==0x200) {
        unsigned reg=(value>>16)&31,operation=(value>>2)&3;
        assert(((value>>21)&31)==PHY_ADDRESS);
        if (operation==1) {
            if (reg==31) page=regs[0][0x204/4];
            else phy[page][reg]=regs[0][0x204/4];
        } else {
            assert(operation==3);
            if (reg==0 && !page && phy[0][0]==0x8000) phy[0][0]=0;
            regs[0][0x204/4]=reg==31?page:phy[page][reg];
        }
        regs[0][offset/4]=0;
    }
}
static void reset(void) {
    memset(regs,0,sizeof(regs));memset(phy,0,sizeof(phy));page=busy=writes=progress=0;time_us=0;
    regs[3][0x60/4]=0x107d;regs[3][0x64/4]=0x1043;regs[3][0xc0/4]=16;regs[3][0x240/4]=7;
    regs[3][0x33c/4]=3;regs[1][0x384/4]=0x8000;regs[0][0x110/4]=0x3051;
    regs[2][0x148/4]=0x20;regs[2][0x140/4]=0x50;regs[2][0x144/4]=2;
    regs[5][0x78/4]=1;
    phy[0][2]=EXPECT_PHY_ID>>16;phy[0][3]=EXPECT_PHY_ID&0xffff;
    phy[0][1]=0x7809;
    phy[0xd08][17]=0xabff;phy[0xd08][21]=0xffff;
}
int main(void) {
    unsigned speed,duplex;
    reset();assert(link_init()==0);assert(progress>60000);
    assert((regs[3][0x33c/4]&3)==3);assert((regs[1][0x384/4]&0x8000)==0x8000);
    assert((phy[0xd08][17]&0x100)==0);assert((phy[0xd08][21]&8)==0);
    assert(phy[0][4]==0x01e1 && phy[0][9]==0x0200 && phy[0][0]==0x1200);
    assert(link_poll(&speed,&duplex)==0);
    phy[0][1]=36;phy[0xa43][26]=0x2c;
    assert(link_poll(&speed,&duplex)==1 && speed==1000 && duplex==1);
    phy[0xa43][26]=0x1c;assert(link_poll(&speed,&duplex)==1 && speed==100);
    phy[0xa43][26]=4;assert(link_poll(&speed,&duplex)==1 && speed==10 && duplex==0);
    phy[0][1]=0x7809;assert(link_poll(&speed,&duplex)==0 && speed==0);
    phy[0][1]=36;phy[0xa43][26]=0x2c;assert(link_poll(&speed,&duplex)==1);
    phy[0xa43][26]=0x3c;assert(link_poll(&speed,&duplex)==-7);
    regs[0][0x1104/4]=1;assert(link_poll(&speed,&duplex)==-3);
    link_stop();assert(regs[3][0x434/4]&0x80);
    assert(!link_allowed(0,0x1104,1));assert(!link_allowed(0,0x1108,1));assert(!link_allowed(0,0,3));
    for (unsigned off=0x1110;off<=0x1130;off+=4) assert(!link_allowed(0,off,0x22000000));
    assert(!link_allowed(3,0x60,0xffffffff));assert(!link_allowed(3,0x33c,0xffff0000));
    reset();phy[0][3]^=1;assert(link_init()==5);
    reset();busy=1;assert(link_init()==4);
    reset();regs[3][0x240/4]=3;assert(link_init()==1 && writes==0);
    reset();regs[0][0x110/4]=0xffffffff;assert(link_init()==2);
    reset();regs[2][0x140/4]=0;assert(link_init()==9 && writes==0);
    reset();regs[5][0x78/4]=0;assert(link_init()==8 && regs[5][12/4]==0);
    return 0;
}
