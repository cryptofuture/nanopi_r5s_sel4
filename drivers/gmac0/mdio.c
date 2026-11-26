/* SPDX-License-Identifier: BSD-2-Clause */
#include "mdio.h"
static int idle(struct mdio_io *io) {
    uint64_t start=io->microseconds(io->context);
    for (unsigned count=0;count<100000;count++) {
        if (!(io->read(io->context,0x200)&1)) return 0;
        if (io->microseconds(io->context)-start>=10000) return -1;
    }
    return -1;
}
int mdio_read(struct mdio_io *io,unsigned phy,unsigned reg,unsigned clock_code,uint16_t *value) {
    if (!io || !io->read || !io->write || !io->microseconds || !value || phy>31 || reg>31 ||
        !(clock_code==1 || clock_code==2 || clock_code==4 || clock_code==5)) return -2;
    if (idle(io)) return -1;
    io->write(io->context,0x200,(phy<<21)|(reg<<16)|(clock_code<<8)|(3<<2)|1);
    if (idle(io)) return -1;
    *value=io->read(io->context,0x204)&0xffff;
    return 0;
}
int mdio_write(struct mdio_io *io,unsigned phy,unsigned reg,unsigned clock_code,uint16_t value) {
    if (!io || !io->read || !io->write || !io->microseconds || phy>31 || reg>31 ||
        !(clock_code==1 || clock_code==2 || clock_code==4 || clock_code==5)) return -2;
    if (idle(io)) return -1;
    io->write(io->context,0x204,value);
    io->write(io->context,0x200,(phy<<21)|(reg<<16)|(clock_code<<8)|(1<<2)|1);
    return idle(io);
}
