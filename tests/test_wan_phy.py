import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

class WanPhy(unittest.TestCase):
    def test_previous_and_corrected_link_poll(self):
        driver = ROOT/'drivers/gmac0'
        prefix = (driver/'link.c').read_text().split('int link_poll(unsigned *speed,unsigned *duplex) {')[0]
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            new = temp/'link-profile.c'
            new.write_text(prefix.replace('#include "mdio.h"', '#include "mdio.h"\n#include "dma/dma.h"') +
                           (driver/'negotiated.c.inc').read_text())
            old, = (ROOT/'out/router-wan-stack/build-a/work/wan-driver/target').rglob('link-profile.c')
            for source in (old,new):
                exe = temp/'poll'
                subprocess.run(['cc','-std=c11','-Werror','-Wall','-Wextra','-fsanitize=address,undefined',
                                '-I',driver,ROOT/'tests/wan_negotiation_test.c',source,driver/'mdio.c','-o',exe],check=True)
                result = subprocess.run([exe], cwd=temp, capture_output=True, text=True, timeout=5)
                if source == old:
                    self.assertNotEqual(result.returncode,0)
                    self.assertIn('speed==1000',result.stderr)
                else:
                    self.assertEqual(result.returncode,0,result.stderr)
