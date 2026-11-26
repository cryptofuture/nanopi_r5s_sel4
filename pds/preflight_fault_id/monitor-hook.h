#ifndef PREFLIGHT_MONITOR_H
#define PREFLIGHT_MONITOR_H
#include <stdint.h>
#define FAULT_WORDS 24
#ifndef FAULT_BUILD_ID
#define FAULT_BUILD_ID {0,0,0,0}
#endif
_Static_assert(sizeof(uint32_t)==4,"mailbox word");
static void publish_fault(uint32_t *wire,uint32_t pd,uint32_t label,const uint64_t args[4],uint64_t ticks) {
 if (__atomic_load_n(wire,__ATOMIC_ACQUIRE)!=0) return;
 __atomic_exchange_n(wire,1,__ATOMIC_ACQ_REL);
 __atomic_store_n(wire+1,1,__ATOMIC_RELAXED);
 __atomic_store_n(wire+2,pd,__ATOMIC_RELAXED);
 __atomic_store_n(wire+3,label,__ATOMIC_RELAXED);
 for (unsigned i=0;i<4;i++) {
  __atomic_store_n(wire+4+2*i,(uint32_t)args[i],__ATOMIC_RELAXED);
  __atomic_store_n(wire+5+2*i,(uint32_t)(args[i]>>32),__ATOMIC_RELAXED);
 }
 __atomic_store_n(wire+12,(uint32_t)ticks,__ATOMIC_RELAXED);
 __atomic_store_n(wire+13,(uint32_t)(ticks>>32),__ATOMIC_RELAXED);
 const uint32_t identity[4]=FAULT_BUILD_ID;
 for (unsigned i=0;i<4;i++) __atomic_store_n(wire+17+i,identity[i],__ATOMIC_RELAXED);
 __atomic_store_n(wire+21,0x50524632,__ATOMIC_RELAXED);
 __atomic_store_n(wire,2,__ATOMIC_RELEASE);
}
static void fault_pc(uint32_t *wire,uint32_t pd,uint64_t pc) {
 if (__atomic_load_n(wire,__ATOMIC_ACQUIRE)!=2 || wire[2]!=pd) return;
 __atomic_exchange_n(wire,3,__ATOMIC_ACQ_REL);
 __atomic_store_n(wire+14,(uint32_t)pc,__ATOMIC_RELAXED);
 __atomic_store_n(wire+15,(uint32_t)(pc>>32),__ATOMIC_RELAXED);
 __atomic_store_n(wire+16,1,__ATOMIC_RELAXED);
 __atomic_store_n(wire,4,__ATOMIC_RELEASE);
}
#endif
