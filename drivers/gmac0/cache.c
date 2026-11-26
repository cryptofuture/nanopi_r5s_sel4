/* SPDX-License-Identifier: BSD-2-Clause */
#include "cache.h"
#ifndef R5S_CACHE_TEST
#include <sel4/sel4.h>
static int flush(unsigned op,uintptr_t start,uintptr_t end) {
    if (op==0) return seL4_ARM_VSpace_Clean_Data(3,start,end);
    if (op==1) return seL4_ARM_VSpace_Invalidate_Data(3,start,end);
    return seL4_ARM_VSpace_CleanInvalidate_Data(3,start,end);
}
#define CACHE_BARRIER() __asm__ volatile("dsb sy" ::: "memory")
#else
int flush(unsigned op,uintptr_t start,uintptr_t end);
#define CACHE_BARRIER() ((void)0)
#endif
int gmac_cache_range(uintptr_t pool,size_t pool_size,uintptr_t address,size_t length,unsigned operation) {
    if (operation>2 || !pool_size || pool%4096 || pool_size%4096 || !length || address%64 || length%64 ||
        address<pool || address-pool>=pool_size || length>pool_size-(address-pool) || pool>UINTPTR_MAX-pool_size) return -1;
    CACHE_BARRIER();
    while (length) {
        size_t part=4096-(address%4096);if (part>length) part=length;
        int error=flush(operation,address,address+part);
        if (error) return error;
        address+=part;length-=part;
    }
    CACHE_BARRIER();return 0;
}
