#include <assert.h>
#include <stdint.h>
#include "link.h"
void fixture_reset(unsigned,unsigned,unsigned);void fixture_settle(void);void fixture_unresolved(void);void fixture_torn(unsigned);void fixture_latched(void);void fixture_busy(unsigned);void fixture_active(void);void fixture_wrong_phy(void);
void host_schedule(uint64_t now) {(void)now;}
void host_progress(uint64_t now) {(void)now;}
int main(void) {
 unsigned speed,duplex;
 fixture_reset(1,1000,1);assert(link_init()==0);fixture_unresolved();
 assert(link_poll(&speed,&duplex)==0); /* Unresolved speed fields must not become a fatal code. */
 fixture_settle();fixture_latched();assert(link_poll(&speed,&duplex)==1 && speed==1000 && duplex==1);
 fixture_torn(1);assert(link_poll(&speed,&duplex)==0);fixture_torn(0);
 for (unsigned i=0;i<5;i++) {unsigned s=i<2?10:i<4?100:1000,d=i%2 || i==4;fixture_reset(1,s,d);assert(link_init()==0);fixture_settle();assert(link_poll(&speed,&duplex)==1 && speed==s && duplex==d);}
 fixture_reset(0,1000,1);assert(link_init()==0);assert(link_poll(&speed,&duplex)==0);
 fixture_reset(1,1000,0);assert(link_init()==0);fixture_settle();assert(link_poll(&speed,&duplex)==-7);
 fixture_reset(1,1000,1);assert(link_init()==0);fixture_wrong_phy();assert(link_poll(&speed,&duplex)==-5);
 fixture_reset(1,1000,1);assert(link_init()==0);fixture_busy(1);assert(link_poll(&speed,&duplex)==-4);
 fixture_reset(1,1000,1);assert(link_init()==0);fixture_active();assert(link_poll(&speed,&duplex)==-3);
 return 0;
}
