int link_inactive(void) {return stopped();}
int link_poll(unsigned *speed,unsigned *duplex) {
    uint16_t status,second,bmsr,hi,lo;
    *speed=*duplex=0;
    if (!stopped()) return -3;
    if (write_phy(31,0) || read_phy(2,&hi) || read_phy(3,&lo) || read_phy(1,&bmsr) || read_phy(1,&bmsr)) return -4;
    link_phy_id=((uint32_t)hi<<16)|lo;
    if (link_phy_id!=EXPECT_PHY_ID || !bmsr || bmsr==0xffff) return -5;
    if ((bmsr&36)!=36) return 0;
    if (write_phy(31,0xa43) || read_phy(26,&status) || read_phy(26,&second) || write_phy(31,0) || read_phy(1,&bmsr) || read_phy(1,&bmsr)) return -4;
    if (!bmsr || bmsr==0xffff) return -5;
    if ((bmsr&36)!=36 || (status&0x804)!=0x804 || ((status^second)&0x83c)!=0) return 0;
    unsigned code=(status>>4)&3;
    if (code==3 || (code==2 && !(status&8))) return -7;
    *speed=code==2?1000:code==1?100:10;*duplex=!!(status&8);
    hw_write(3,0x17c,0x00300000|(*speed==1000?0:*speed==100?0x30:0x20));
    uint32_t config=hw_read(0,0)&~((1u<<13)|(1u<<14)|(1u<<15)|3u);
    config|=(*duplex?1u<<13:0)|(*speed!=1000?1u<<15:0)|(*speed==100?1u<<14:0);
    hw_write(0,0,config);
    return stopped()?1:-3;
}
