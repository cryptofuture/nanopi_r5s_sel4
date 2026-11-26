#define hw_write base_write
#define main old_link_test
#include "gmac_link_test.c"
#undef main
#undef hw_write
static int latch;
void hw_write(unsigned region,unsigned offset,uint32_t value) {
    base_write(region,offset,value);
    if (latch && !region && offset==0x200 && ((value>>16)&31)==1 && ((value>>2)&3)==3) {
        latch=0;regs[0][0x204/4]=0x7809;
    }
}

int dma_enabled(void) {return 0;}
int dma_prepare_link(unsigned speed, unsigned duplex) {(void)speed; (void)duplex; return 0;}

int main(void) {
    unsigned speed, duplex;
    reset();
    assert(link_init()==0);
    phy[0][1]=0x7809;
    assert(link_poll(&speed,&duplex)==0);
    phy[0][1]=0x782d;
    phy[0][5]=0x41e1;
    phy[0][10]=0x3800;
    phy[0xa43][26]=0x002c; latch=1;
    assert(link_poll(&speed,&duplex)==1 && speed==1000 && duplex==1);
    phy[0][10]=0;
    assert(link_poll(&speed,&duplex)==0);
    phy[0xa43][26]=0x001c;
    assert(link_poll(&speed,&duplex)==1 && speed==100 && duplex==1);
    phy[0][1]=0x7809;
    assert(link_poll(&speed,&duplex)==0);
    phy[0][1]=0x782d;
    phy[0][10]=0x3800;
    phy[0xa43][26]=0x082c;
    assert(link_poll(&speed,&duplex)==1 && speed==1000);
    phy[0xa43][26]=0xffff;
    assert(link_poll(&speed,&duplex)<0);
    busy=1;
    assert(link_poll(&speed,&duplex)==-4);
}
