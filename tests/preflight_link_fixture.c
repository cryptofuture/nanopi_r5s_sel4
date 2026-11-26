#define main original_fixture_main
#define hw_us fixture_base_us
#define hw_progress fixture_base_progress
#define hw_read fixture_base_read
#define hw_write fixture_base_write
#include "gmac_link_test.c"
#undef main
#undef hw_us
#undef hw_progress
#undef hw_read
#undef hw_write
static uint64_t wall,cpu,negotiated_at;
static unsigned scenario,rate=1000,half,transient,latched,status_reads;
extern void host_schedule(uint64_t now);
extern void host_progress(uint64_t now);
void fixture_advance(unsigned units) {
 for (unsigned i=0;i<units;i++) {wall++;cpu++;if (cpu%200==0) wall+=800;host_schedule(wall/1000);}
}
uint64_t hw_us(void) {fixture_advance(1);return wall;}
void hw_progress(void) {host_progress(wall/1000);}
uint32_t hw_read(unsigned region,unsigned offset) {return fixture_base_read(region,offset);}
static unsigned cable(void) {
 uint64_t ms=wall/1000;
 switch (scenario) {case 0:return 0;case 1:return 1;case 2:return ms>=200;case 3:return ms>=1400;case 4:return ms>=2200;case 5:return ms<6000 || ms>=10000;case 6:return ms<200 || ms>=5000;case 7:return ms<1400 || ms>=5000;case 8:return ms<10 || ms>=5000;case 9:return ms>=10;case 10:return ms<1405 || ms>=5000;case 11:return ms>=1450;default:return 1;}
}
void hw_write(unsigned region,unsigned offset,uint32_t value) {
 if (!region && offset==0x200 && ((value>>2)&3)==3) {
  unsigned reg=(value>>16)&31;
  if (reg==1 && page==0) {
   phy[0][1]=cable()?(wall>=negotiated_at?0x782d:0x782d):0x7809;
   if (latched) {phy[0][1]=0x7809;latched=0;}
  }
  if (reg==26 && page==0xa43) {
   status_reads++;
   phy[page][reg]=!cable()?0:wall<negotiated_at?0x3c:(uint16_t)(0x804|(rate==1000?32:rate==100?16:0)|(half?0:8));
   if (transient && status_reads%2==0) phy[page][reg]^=0x10;
  }
 }
 if (!region && offset==0x200 && ((value>>2)&3)==1 && ((value>>16)&31)==0 && regs[0][0x204/4]==0x1200) negotiated_at=wall+(scenario==12?8000000:300000);
 fixture_base_write(region,offset,value);
 assert((regs[0][0]&3)==0 && (regs[0][0x1104/4]&1)==0 && (regs[0][0x1108/4]&1)==0);
 for (unsigned off=0x1110;off<=0x111c;off+=4) assert(regs[0][off/4]==0);
}
void fixture_reset(unsigned which,unsigned speed,unsigned duplex) {reset();wall=cpu=negotiated_at=0;scenario=which;rate=speed;half=!duplex;transient=latched=status_reads=0;}
void fixture_settle(void) {negotiated_at=0;}
void fixture_unresolved(void) {negotiated_at=wall+10000000;}
void fixture_torn(unsigned enabled) {transient=enabled;status_reads=0;}
void fixture_latched(void) {latched=1;}
void fixture_busy(unsigned enabled) {busy=enabled;}
void fixture_active(void) {regs[0][0x1104/4]=1;}
void fixture_wrong_phy(void) {phy[0][3]^=1;}
uint64_t fixture_wall(void) {return wall/1000;}
