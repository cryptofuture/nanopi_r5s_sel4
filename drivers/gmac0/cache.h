/* SPDX-License-Identifier: BSD-2-Clause */
#ifndef R5S_DMA_CACHE_H
#define R5S_DMA_CACHE_H
#include <stddef.h>
#include <stdint.h>
int gmac_cache_range(uintptr_t pool,size_t pool_size,uintptr_t address,size_t length,unsigned operation);
#endif
