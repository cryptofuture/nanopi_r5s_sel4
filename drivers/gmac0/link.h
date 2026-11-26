/* SPDX-License-Identifier: BSD-2-Clause */
#ifndef R5S_LINK_H
#define R5S_LINK_H
#include <stdint.h>
uint32_t hw_read(unsigned region,unsigned offset);
void hw_write(unsigned region,unsigned offset,uint32_t value);
uint64_t hw_us(void);
void hw_progress(void);
int link_allowed(unsigned region,unsigned offset,uint32_t value);
int link_init(void);
void link_stop(void);
int link_poll(unsigned *speed,unsigned *duplex);
extern uint32_t link_mac_version,link_phy_id,link_clock_hz;
#endif
