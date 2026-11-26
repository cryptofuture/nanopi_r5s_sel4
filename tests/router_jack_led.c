#define main old_main
#include "gmac_link_test.c"
#undef main
int link_configure_jack_leds(void);
int main(void) {
    reset();
    assert(link_init()==0);
    phy[0xd04][16]=0x001b;
    assert(link_configure_jack_leds()==1 && page==0);
    assert(phy[0xd04][16]==(0x8000|0x0200|(0xb<<10)|0x1b));
    assert(phy[0xd08][17]==(0xabff & ~0x100));
    phy[0xd04][16]=0xffff;
    assert(link_configure_jack_leds()==0 && page==0 && phy[0xd04][16]==0xffff);
    busy=1;assert(link_configure_jack_leds()==0);
    return 0;
}
