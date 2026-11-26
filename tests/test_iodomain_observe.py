import subprocess,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def function(text,name):
 start=text.index(name);start=text.rfind('\n',0,start)+1;op=text.index('{',start);depth=1;end=op+1
 while depth:
  depth+=(text[end]=='{')-(text[end]=='}');end+=1
 return text[start:end]
class IODomainObserve(unittest.TestCase):
 def test_actual_probe(self):
  text=(ROOT/'out/iodomain-observe/work/u-boot/drivers/misc/rockchip-io-domain.c').read_text()
  declarations='''
#include <stdint.h>
#include <string.h>
#include <assert.h>
#include <errno.h>
typedef uint32_t u32;typedef unsigned uint;
#define BIT(n) (1u<<(n))
#define MAX_VOLTAGE_1_8 1980000
#define MAX_VOLTAGE_3_3 3600000
#define MAX_SUPPLIES 16
#define RK3568_PMU_GRF_IO_VSEL0 0x140
#define RK3568_PMU_GRF_IO_VSEL1 0x144
#define RK3568_PMU_GRF_IO_VSEL2 0x148
#define IS_ERR(x) 0
#define PTR_ERR(x) -1
#define dev_dbg(...) ((void)0)
#define dev_crit(...) ((void)0)
#define dev_err(...) ((void)0)
struct regmap {unsigned regs[128];};struct udevice {const char *name;};
struct rockchip_iodomain_soc_data {const char *supply_names[16];unsigned grf_offset;int (*write)(struct regmap*,uint,int,int);};
static struct rockchip_iodomain_soc_data soc;static struct regmap map;static struct udevice regulator;
static int failure,selected,writes;
static void *dev_get_driver_data(struct udevice *d) {(void)d;return &soc;}
static struct udevice *dev_get_parent(struct udevice *d) {return d;}
static struct regmap *syscon_get_regmap(struct udevice *d) {(void)d;return &map;}
static int device_get_supply_regulator(struct udevice *d,const char *s,struct udevice **r) {(void)d;selected=*s-'0';*r=&regulator;return failure==1?-19:0;}
static int regulator_autoset(struct udevice *d) {(void)d;return failure==2?-5:0;}
static int regulator_get_value(struct udevice *d) {(void)d;return failure==3?-5:selected<3?3300000:1800000;}
static int regmap_write(struct regmap *m,unsigned a,unsigned v) {writes++;if(failure==4)return -5;unsigned mask=v>>16;m->regs[a/4]=(m->regs[a/4]&~mask)|(v&mask);return 0;}
static int regmap_read(struct regmap *m,unsigned a,unsigned *v) {*v=m->regs[a/4]^(failure==5?255:0);return 0;}
u32 r5s_io_results[4][10];
'''
  code=declarations+'\n'+'\n'.join(function(text,n) for n in ['r5s_io_slot(','r5s_io_row(','rk3568_iodomain_write(','rockchip_iodomain_probe('])
  code+='''
int main(void) {
 soc.supply_names[1]="1";soc.supply_names[2]="2";soc.supply_names[5]="5";soc.supply_names[7]="7";soc.write=rk3568_iodomain_write;
 for(failure=0;failure<=5;failure++) {
  memset(r5s_io_results,0,sizeof(r5s_io_results));memset(&map,0,sizeof(map));writes=0;
  assert(rockchip_iodomain_probe(&regulator)==0);
  for(int i=0;i<4;i++) {
   assert(r5s_io_results[i][0]==(failure==1?2:failure==2?3:failure==3?4:5));
   if(failure==1)assert((int)r5s_io_results[i][1]==-19);
   if(failure==2)assert((int)r5s_io_results[i][2]==-5);
   if(failure==3)assert((int)r5s_io_results[i][3]==-5);
   if(failure==4)assert((int)r5s_io_results[i][4]==-5 && (int)r5s_io_results[i][5]==-5);
  }
  assert(writes==((failure>=1 && failure<=3)?0:8));
  if(!failure) {assert((map.regs[0x148/4]&0x22)==0x20);assert((map.regs[0x140/4]&0x52)==0x50);assert((map.regs[0x144/4]&0x52)==2);}
 }
}
'''
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'test.c').write_text(code)
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-parameter','-fsanitize=address,undefined',p/'test.c','-o',p/'test'],check=True)
   subprocess.run([p/'test'],check=True)
 def test_actual_observation(self):
  with tempfile.TemporaryDirectory() as tmp:
   exe=Path(tmp)/'test';subprocess.run(['rustc','--edition','2021','-A','dead_code','--test',ROOT/'tests/iodomain_observe_test.rs','-o',exe],check=True);subprocess.run([exe],check=True)
if __name__=='__main__':unittest.main()
