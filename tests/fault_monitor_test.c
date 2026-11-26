#include <assert.h>
#include "../pds/preflight_fault_id/monitor-hook.h"
int main(void) {
 uint32_t w[FAULT_WORDS]={0};uint64_t args[4]={0x1234567887654321ull,2,3,4};
 publish_fault(w,2,6,args,99);assert(w[0]==2 && w[1]==1 && w[2]==2 && w[3]==6);
 assert(w[4]==0x87654321u && w[5]==0x12345678u && w[12]==99);
 publish_fault(w,1,8,args,100);assert(w[2]==2 && w[3]==6 && w[12]==99);
 fault_pc(w,2,0xabcdef0012345678ull);assert(w[0]==4 && w[14]==0x12345678 && w[15]==0xabcdef00 && w[16]==1);
 return 0;
}
