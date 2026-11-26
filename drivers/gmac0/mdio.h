/* SPDX-License-Identifier: BSD-2-Clause */
#ifndef R5S_MDIO_H
#define R5S_MDIO_H
#include <stdint.h>
struct mdio_io {
    void *context;
    uint32_t (*read)(void *, unsigned);
    void (*write)(void *, unsigned, uint32_t);
    uint64_t (*microseconds)(void *);
};
int mdio_read(struct mdio_io *, unsigned phy, unsigned reg, unsigned clock_code, uint16_t *value);
int mdio_write(struct mdio_io *, unsigned phy, unsigned reg, unsigned clock_code, uint16_t value);
#endif
