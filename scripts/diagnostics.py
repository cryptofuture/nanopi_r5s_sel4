#!/usr/bin/env python3
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
SPEC = json.loads((ROOT / 'protocol/diagnostics-v2.json').read_text())

def rust():
    arrays = '\n'.join('const ' + name + ': &[(bool,u64)] = &[' + ','.join(f'({str(on).lower()},{ms})' for on, ms in pattern) + '];' for name, pattern in SPEC['states'].items())
    return '''#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State { Wait, Ready, IoVerified, Fault }
''' + arrays + f'''
pub const HEALTH_MS:u64={SPEC['health_deadline_ms']};
pub const LEASE_MS:u64={SPEC['lease_ms']};
pub const POLL_MS:u64={SPEC['poll_deadline_ms']};
pub fn select(now:u64, health:u64, link:bool, proof:Option<u64>, fault:bool)->State {{
    if fault || now.saturating_sub(health)>HEALTH_MS {{State::Fault}}
    else if !link {{State::Wait}}
    else if proof.is_some_and(|t| t<=now && now-t<LEASE_MS) {{State::IoVerified}}
    else {{State::Ready}}
}}
pub struct Display {{pub state:State, start:u64}}
impl Display {{
    pub fn new(now:u64)->Self {{Self{{state:State::Wait,start:now}}}}
    pub fn tick(&mut self, state:State, now:u64)->bool {{
        if state!=self.state {{self.state=state;self.start=now;}}
        let elapsed=now.saturating_sub(self.start);
        if elapsed<{SPEC['delimiter_ms']} {{return false;}}
        let pattern=match state {{State::Wait=>WAIT,State::Ready=>READY,State::IoVerified=>IO_VERIFIED,State::Fault=>FAULT}};
        let mut position=(elapsed-{SPEC['delimiter_ms']})%pattern.iter().map(|p|p.1).sum::<u64>();
        for &(on,duration) in pattern {{if position<duration {{return on;}} position-=duration;}}
        false
    }}
}}
'''

def generate():
    (ROOT / 'pds/diagnostics/src/display.rs').write_text(rust())
    lines=['# Diagnostics v2','', 'Generated from `protocol/diagnostics-v2.json`. Durations are nominal milliseconds.', '', '| State | ON/OFF sequence |','|---|---|']
    for name, pattern in SPEC['states'].items():
        lines.append('| '+name+' | '+'; '.join(('ON ' if on else 'OFF ')+str(ms) for on,ms in pattern)+' |')
    lines += ['', 'Every state change starts with 500 ms OFF. The FAULT sequence includes the final 125 ms OFF plus 4000 ms darkness.', '', 'State selection runs every scheduler opportunity, before pattern progression. Required health expires after 100 ms; proof expires at 5000 ms. Expected scheduling bound is 10 ms, including the delimiter gives at most 510 ms to the first fault indication. A scheduler/kernel failure can freeze the LED. Partial frames, solid ON and solid OFF are inconclusive.', '', 'READY means profile checks pass, never packet proof. IO_VERIFIED requires a matching fresh external ACK and is revoked on link loss, health failure or lease expiry. FAULT takes priority. Baseline has no IO_VERIFIED path.', '', 'Timeline preview: each row below is one period; `#` is ON and `.` is OFF, one character per 125 ms (rounded for preview only).', '']
    for name, pattern in SPEC['states'].items():
        lines.append('    '+name.ljust(12)+''.join(('#' if on else '.')*round(ms/125) for on,ms in pattern))
    (ROOT / 'docs/diagnostics-v2.md').write_text('\n'.join(lines)+'\n')

if __name__=='__main__': generate()
