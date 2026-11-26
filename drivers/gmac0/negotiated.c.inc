/* SPDX-License-Identifier: BSD-2-Clause */
uint32_t link_sample[6];
int link_inactive(void) {return stopped();}

int link_poll(unsigned *speed, unsigned *duplex) {
    uint16_t hi, lo, bmcr, initial, bmsr, adv, partner, gig, peer, first, second;
    *speed = *duplex = 0;
    if (!dma_enabled() && !stopped()) return -3;
    if (write_phy(31, 0) || read_phy(2, &hi) || read_phy(3, &lo) ||
        read_phy(0, &bmcr) || read_phy(1, &initial) || read_phy(1, &bmsr)) return -4;
    link_phy_id = ((uint32_t)hi << 16) | lo;
    if (link_phy_id != EXPECT_PHY_ID || !bmsr || bmsr == 0xffff ||
        bmcr == 0xffff || !(bmcr & 0x1000)) return -5;
    if ((bmsr & 0x24) != 0x24) {
        link_sample[0] = ((uint32_t)bmcr << 16) | initial;
        link_sample[1] = bmsr;
        for (unsigned i=2;i<6;i++) link_sample[i]=0;
        return 0;
    }
    if (read_phy(4, &adv) || read_phy(5, &partner) ||
        read_phy(9, &gig) || read_phy(10, &peer) || write_phy(31, 0xa43)) return -4;
    int error = read_phy(26, &first) || read_phy(26, &second);
    if (write_phy(31, 0) || error || read_phy(1, &bmsr)) return -4;
    if (!bmsr || bmsr == 0xffff || first == 0xffff || second == 0xffff ||
        adv == 0xffff || partner == 0xffff || gig == 0xffff || peer == 0xffff) return -5;
    link_sample[0] = ((uint32_t)bmcr << 16) | initial;
    link_sample[1] = bmsr;
    link_sample[2] = ((uint32_t)adv << 16) | partner;
    link_sample[3] = ((uint32_t)gig << 16) | peer;
    link_sample[4] = ((uint32_t)first << 16) | second;
    link_sample[5] = 1;
    if ((bmsr & 0x24) != 0x24 || !(second & 4) || ((first ^ second) & 0x3c)) return 0;
    unsigned code = (second >> 4) & 3, full = !!(second & 8);
    if (code == 3 || (code == 2 && !full)) return -7;
    unsigned common = adv & partner;
    if (code == 2 ? !(gig & 0x200 && peer & 0x800 && !(peer & 0x8000)) :
        !(common & (code ? (full ? 0x100 : 0x80) : (full ? 0x40 : 0x20)))) {link_sample[5]=2;return 0;}
    *speed = code == 2 ? 1000 : code == 1 ? 100 : 10;
    *duplex = full;
    int change = dma_prepare_link(*speed, *duplex);
    if (change) return change;
    if (dma_enabled()) return 1;
    hw_write(3, 0x17c, 0x00300000 | (code == 2 ? 0 : code ? 0x30 : 0x20));
    uint32_t config = hw_read(0, 0) & ~((7u << 13) | 3u);
    hw_write(0, 0, config | (full << 13) | ((code != 2) << 15) | ((code == 1) << 14));
    return stopped() ? 1 : -3;
}
