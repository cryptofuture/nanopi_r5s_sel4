import sys,subprocess,tempfile,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from test_iodomain_observe import function
class BootPolicy(unittest.TestCase):
 def test_actual_autoset_old_and_captured_policy(self):
  source=(ROOT/'vendor/u-boot/drivers/power/regulator/regulator-uclass.c').read_text()
  body=function(source,'regulator_autoset(')
  code='''
#include <errno.h>
#include <stdbool.h>
#include <assert.h>
#define REGULATOR_FLAG_AUTOSET_DONE 1
#define REGULATOR_FLAG_AUTOSET_UV 2
#define REGULATOR_FLAG_AUTOSET_UA 4
#define REGULATOR_TYPE_FIXED 1
struct dm_regulator_uclass_plat {int flags,suspend_on,suspend_uV,force_off,always_on,boot_on,type,min_uV,init_uV,min_uA;};
struct udevice {int unused;};
static struct dm_regulator_uclass_plat platform;
static int voltage,enabled,writes,failure;
static void *dev_get_uclass_plat(struct udevice *d) {(void)d;return &platform;}
static int regulator_set_suspend_enable(struct udevice *d,int v) {(void)d;(void)v;return 0;}
static int regulator_set_suspend_value(struct udevice *d,int v) {(void)d;(void)v;assert(0);return 0;}
static int regulator_set_enable(struct udevice *d,int v) {(void)d;enabled=v;return 0;}
static int regulator_set_value(struct udevice *d,int v) {(void)d;writes++;if(failure)return -EIO;voltage=v;return 0;}
static int regulator_set_current(struct udevice *d,int v) {(void)d;(void)v;assert(0);return 0;}
'''+body+'''
int main(void) {
 struct udevice dev={0};
 for(int fixed=0;fixed<=1;fixed++) {
  platform=(struct dm_regulator_uclass_plat){.flags=REGULATOR_FLAG_AUTOSET_UV,.min_uV=3300000,.boot_on=fixed,.always_on=fixed};
  voltage=1800000;writes=enabled=0;
  int result=regulator_autoset(&dev);
  assert(result==(fixed?0:-EMEDIUMTYPE));assert(writes==fixed);assert(enabled==fixed);
  assert(voltage==(fixed?3300000:1800000));
  unsigned sel18=voltage<=1980000,sel33=!sel18;
  unsigned encoded=2+24+(sel18<<2)+(sel33<<5);
  assert(encoded==(fixed?58:30));
  assert(regulator_autoset(&dev)==-EALREADY);assert(writes==fixed);
 }
 platform.flags=REGULATOR_FLAG_AUTOSET_UV;voltage=1800000;failure=1;enabled=0;
 assert(regulator_autoset(&dev)==-EIO);assert(voltage==1800000 && enabled==0);
}
'''
  with tempfile.TemporaryDirectory() as folder:
   p=Path(folder);(p/'test.c').write_text(code)
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-fsanitize=address,undefined',p/'test.c','-o',p/'test'],check=True)
   subprocess.run([p/'test'],check=True)
 def test_actual_ldo_voltage_access(self):
  text=(ROOT/'vendor/u-boot/drivers/power/regulator/rk8xx.c').read_text()
  code="""
#include <assert.h>
#include <errno.h>
#define NA 255
#define debug(...) ((void)0)
struct udevice {int unused;};
struct rk8xx_reg_info {int min_uv,step_uv,vsel_reg,vsel_mask,min_sel;};
static unsigned registers[256];static int fail;
static int pmic_reg_read(struct udevice *d,int r) {(void)d;return fail?-EIO:(int)registers[r];}
static int pmic_clrsetbits(struct udevice *d,int r,int mask,int val) {(void)d;assert(r==0xd2 && mask==0x7f);if(fail)return -EIO;registers[r]=(registers[r]&~mask)|val;return 0;}
"""+function(text,'_ldo_set_value(')+'\n'+function(text,'_ldo_get_value(')+"""
int main(void) {
 struct udevice d={0};struct rk8xx_reg_info info={600000,25000,0xd2,0x7f,0};
 for(int i=0;i<256;i++)registers[i]=0xa5;
 registers[0xd2]=0xb0;assert(_ldo_get_value(&d,&info)==1800000);
 assert(_ldo_set_value(&d,&info,3300000)==0);assert(registers[0xd2]==0xec);
 assert(_ldo_get_value(&d,&info)==3300000);
 for(int i=0;i<256;i++)if(i!=0xd2)assert(registers[i]==0xa5);
 fail=1;assert(_ldo_get_value(&d,&info)==-EIO);assert(_ldo_set_value(&d,&info,3300000)==-EIO);
}
"""
  with tempfile.TemporaryDirectory() as folder:
   p=Path(folder);(p/'ldo.c').write_text(code)
   subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-fsanitize=address,undefined',p/'ldo.c','-o',p/'ldo'],check=True)
   subprocess.run([p/'ldo'],check=True)
 def test_captured_policy(self):
  captured=(ROOT/'out/gmac-bringup/runtime-original.dts').read_text().split('LDO_REG4 {',1)[1].split('regulator-state-mem',1)[0]
  assert 'regulator-always-on;' in captured and 'regulator-boot-on;' in captured
  assert captured.count('0x325aa0')==2
  overlay=(ROOT/'firmware/vccio1-boot-policy.dtsi').read_text()
  self.assertEqual(overlay,'&vccio_acodec {\n    regulator-boot-on;\n    regulator-always-on;\n};\n')
if __name__=='__main__':unittest.main()
