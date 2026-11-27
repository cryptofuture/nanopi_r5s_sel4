// Page sections below the hero: download, flash procedure, architecture, performance.
// Colours come from the tokens the hero injects at :root (silkscreen paper, board ink, amber).

const PRODUCTION = {
  name: 'r5s-rtl8125-lan-repair.img',
  role: 'Production',
  note: 'Quiet firmware, release SDK. The image to run.',
  size: '96 MiB',
  sha: '734920599454fff1c447821ce594be0fbd3d14dc6de2312e67fb18ba45a65a1d'
}

const DEBUG = {
  name: 'r5s-rtl8125-lan-repair-debug.img',
  role: 'Debug',
  note: 'Firmware logging, status endpoint, LED fault codes. The image to diagnose with.',
  size: '96 MiB',
  sha: '93a9c30bb655b720a91d2b0b81fef1404fd7f2025c9c17c21bc245cafc9a28e5'
}

const TOOLS = {
  name: 'tools.tar.gz',
  role: 'Tools',
  note: 'Image verification and guarded flashing utilities. Needed for the flash steps.',
  size: '412 KiB',
  sha: '4877c1e93b561e51ea65d9b6f1ea3fe381a75a8b4b487c91072146adfacea280'
}

const Section = ({ id, title, lead, children }) => (
  <section id={id} className='scroll-mt-24 px-6 py-16 sm:px-10 lg:px-14 lg:py-20'>
    <div className='mx-auto max-w-4xl'>
      <h2 className='text-(color:--color-n3-ink) text-[clamp(1.75rem,1.2rem+1.8vw,2.5rem)] leading-tight font-bold tracking-[-0.02em]'>
        {title}
      </h2>
      {lead && (
        <p className='text-(color:--color-n3-grey) mt-4 max-w-[60ch] text-[17px] leading-[1.6]'>{lead}</p>
      )}
      <div className='mt-10'>{children}</div>
    </div>
  </section>
)

const CodeBlock = ({ children }) => (
  <pre className='font-mono-plex overflow-x-auto rounded-2xl bg-[#10191c] px-5 py-4 text-[13px] leading-[1.7] text-[#dfe8e3]'>
    <code>{children}</code>
  </pre>
)

const HashLine = ({ sha }) => (
  <code className='font-mono-plex text-(color:--color-n3-grey) block overflow-x-auto text-[12px] break-all'>{sha}</code>
)

const DownloadRow = ({ image }) => (
  <div className='flex flex-col gap-4 border-t border-(color:--color-n3-line) py-6 sm:flex-row sm:items-center sm:justify-between'>
    <div className='min-w-0'>
      <p className='text-(color:--color-n3-ink) text-[17px] font-semibold'>
        {image.role}
        <span className='text-(color:--color-n3-grey) ml-3 text-[13px] font-normal'>{image.size}</span>
      </p>
      <p className='text-(color:--color-n3-grey) mt-1 text-[14px] leading-[1.5]'>{image.note}</p>
      <p className='font-mono-plex text-(color:--color-n3-ink) mt-2 text-[12.5px] break-all'>{image.name}</p>
      <HashLine sha={image.sha} />
    </div>
    <a
      href={`/downloads/${image.name}`}
      download
      className='n3-btn bg-(color:--color-n3-accent) focus-visible:outline-(color:--color-n3-ink) inline-flex h-12 shrink-0 items-center gap-2 rounded-full px-6 text-[15px] font-medium text-white focus-visible:outline-2 focus-visible:outline-offset-2'
    >
      Download
      <svg
        className='n3-btn-icon'
        width='15'
        height='15'
        viewBox='0 0 24 24'
        fill='none'
        stroke='currentColor'
        strokeWidth='1.8'
        aria-hidden
      >
        <path d='M12 3v12m0 0 5-5m-5 5-5-5M4 21h16' strokeLinecap='round' strokeLinejoin='round' />
      </svg>
    </a>
  </div>
)

export const Download = () => (
  <Section
    id='download'
    title='Download'
    lead='Two images of the same firmware and the tool archive the flash steps use. Run production, diagnose with debug. Check the SHA-256 after downloading.'
  >
    <div className='border-b border-(color:--color-n3-line)'>
      <DownloadRow image={PRODUCTION} />
      <DownloadRow image={DEBUG} />
      <DownloadRow image={TOOLS} />
    </div>
    <div className='mt-8'>
      <p className='text-(color:--color-n3-ink) text-[15px] font-semibold'>Verify what you downloaded</p>
      <p className='text-(color:--color-n3-grey) mt-2 max-w-[60ch] text-[15px] leading-[1.6]'>
        Keep the files in one folder together with the SHA256SUMS list from this page:
      </p>
      <div className='mt-4'>
        <CodeBlock>sha256sum -c --ignore-missing SHA256SUMS</CodeBlock>
      </div>
    </div>
  </Section>
)

const Step = ({ n, title, children, body }) => (
  <div className='mt-10 first:mt-0'>
    <div className='flex items-baseline gap-3'>
      <span className='font-mono-plex text-(color:--color-n3-accent) text-[13px] font-medium'>{n}</span>
      <h3 className='text-(color:--color-n3-ink) text-[19px] font-semibold tracking-[-0.01em]'>{title}</h3>
    </div>
    {body && (
      <p className='text-(color:--color-n3-grey) mt-3 max-w-[62ch] text-[15px] leading-[1.65]'>{body}</p>
    )}
    <div className='mt-4'>{children}</div>
  </div>
)

export const Flash = () => (
  <Section
    id='flash'
    title='Flash the SD card'
    lead='The firmware runs from an SD card. Whatever Linux your R5S currently runs (usually on its eMMC) stays untouched and remains available as recovery. The steps below copy the image into the device’s Linux, verify it there, and write it to the SD card. Afterwards the board boots the firmware from the card.'
  >
    <Step
      n='01'
      title='Stage the downloads, then copy them to the R5S'
      body='Run on your own Linux machine, in the folder where you downloaded the image, tools.tar.gz and SHA256SUMS. Replace 10.0.1.81 with your R5S’s LAN address. For the debug image, set IMAGE to the debug filename.'
    >
      <CodeBlock>
        {`IMAGE="r5s-rtl8125-lan-repair.img"
STAGE=/tmp/r5s-wan-fastpath

mkdir -p "$STAGE"

cp "$IMAGE" tools.tar.gz SHA256SUMS "$STAGE/"

ssh root@10.0.1.81 \\
  'mkdir -p /dev/shm/r5s-wan-fastpath'

scp "$STAGE/"* \\
  root@10.0.1.81:/dev/shm/r5s-wan-fastpath/`}
      </CodeBlock>
    </Step>

    <Step
      n='02'
      title='On the R5S: check the copies and verify the image'
      body='Log into the device’s Linux and run the checks. Nothing is written to any disk in this step.'
    >
      <CodeBlock>
        {`ssh root@10.0.1.81
cd /dev/shm/r5s-wan-fastpath

export TMPDIR="$PWD"

sha256sum -c --ignore-missing SHA256SUMS &&
tar -xzf tools.tar.gz &&
python3 scripts/verify-image r5s-rtl8125-lan-repair.img`}
      </CodeBlock>
    </Step>

    <Step
      n='03'
      title='Identify the SD card'
      body='Still on the R5S. Check which block device is the card you want to write, and read its identity. The name becomes SD_MODEL, the CID becomes SD_CID, and the size in bytes becomes SD_BYTES.'
    >
      <CodeBlock>
        {`findmnt /

lsblk -b -o NAME,PATH,TYPE,SIZE,MODEL,MOUNTPOINTS

cat /sys/class/block/mmcblk0/device/type
cat /sys/class/block/mmcblk0/device/name
cat /sys/class/block/mmcblk0/device/cid

blockdev --getsize64 /dev/mmcblk0`}
      </CodeBlock>
    </Step>

    <Step
      n='04'
      title='Write the SD card'
      body='Set the SD_* guards to the values you just read. The ones below are one working example. The first command is a dry run; the second, with --execute, writes the card.'
    >
      <CodeBlock>
        {`export SD_DEVICE=/dev/mmcblk0
export SD_MODEL=NCard
export SD_CID=8903034e43617264100d3aa313016c07
export SD_BYTES=62534975488

export IMAGE=r5s-rtl8125-lan-repair.img
python3 scripts/flash-sd \\
  --profile r5s \\
  --device "\${SD_DEVICE:?}" \\
  --image "\${IMAGE:?}" \\
  --expect-model "\${SD_MODEL:?}" \\
  --expect-serial "\${SD_CID:?}" \\
  --expect-bytes "\${SD_BYTES:?}"

python3 scripts/flash-sd \\
  --profile r5s \\
  --device "\${SD_DEVICE:?}" \\
  --image "\${IMAGE:?}" \\
  --expect-model "\${SD_MODEL:?}" \\
  --expect-serial "\${SD_CID:?}" \\
  --expect-bytes "\${SD_BYTES:?}" \\
  --execute`}
      </CodeBlock>
    </Step>

    <div className='mt-10 rounded-2xl border border-(color:--color-n3-line) bg-white/60 px-6 py-5'>
      <p className='text-(color:--color-n3-ink) text-[15px] font-semibold'>If you flashed the debug image</p>
      <p className='text-(color:--color-n3-grey) mt-2 max-w-[62ch] text-[15px] leading-[1.65]'>
        It adds firmware logging, full LED fault codes, the port-5201 TSO test generator and a
        read-only status endpoint on port 48125. Ask it for a full JSON report:
      </p>
      <div className='mt-4'>
        <CodeBlock>
          {'python3 scripts/read-b51-udp <router-ip>'}
        </CodeBlock>
      </div>
      <p className='text-(color:--color-n3-grey) mt-3 max-w-[62ch] text-[15px] leading-[1.65]'>
        Measure throughput on the production image. The debug SDK and logging cost real cycles.
      </p>
    </div>
  </Section>
)

const PD_ROWS = [
  ['gmac', '1', '710', 'GMAC0 rings, TSO, checksum offload'],
  ['heartbeat', '1', '200', 'supervisor, SYS LED'],
  ['health', '1', '90', 'health monitor'],
  ['logical_lan', '0', '330', 'dispatcher, bridge, DHCP, fallback'],
  ['wan', '0', '130', 'DHCP client, ARP, echo'],
  ['forward0', '0', '260', 'NAT worker'],
  ['forward1', '0', '220', 'NAT worker'],
  ['lan1', '2', '650', 'RTL8125 rings, GRO'],
  ['forward2', '2', '300', 'NAT worker (LAN 1)'],
  ['lan2', '3', '650', 'RTL8125 rings, GRO'],
  ['forward3', '3', '300', 'NAT worker (LAN 2)']
]

export const Architecture = () => (
  <Section
    id='architecture'
    title='Architecture'
    lead='A seL4/Microkit system of eleven protection domains on four Cortex-A55 cores. Payloads never move between them: a shared zero-copy DMA pool hands off 16-byte references. Budgets are microseconds per 1 ms.'
  >
    <div className='overflow-x-auto'>
      <table className='w-full border-collapse text-left'>
        <thead>
          <tr className='border-b border-(color:--color-n3-ink)/25'>
            <th className='py-2 pr-4 text-[13px] font-semibold'>PD</th>
            <th className='py-2 pr-4 text-[13px] font-semibold'>Core</th>
            <th className='py-2 pr-4 text-[13px] font-semibold'>µs / 1 ms</th>
            <th className='py-2 text-[13px] font-semibold'>Role</th>
          </tr>
        </thead>
        <tbody>
          {PD_ROWS.map(([pd, core, budget, role]) => (
            <tr key={pd} className='border-b border-(color:--color-n3-line)'>
              <td className='font-mono-plex py-2.5 pr-4 text-[13px]'>{pd}</td>
              <td className='py-2.5 pr-4 text-[14px] tabular-nums'>{core}</td>
              <td className='py-2.5 pr-4 text-[14px] tabular-nums'>{budget}</td>
              <td className='text-(color:--color-n3-grey) py-2.5 text-[14px]'>{role}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>

    <div className='mt-10'>
      <CodeBlock>
        {`download   gmac RX → worker (NAT + frame prep) → lanX TX → client
upload     client → lanX RX → worker (NAT + frame prep) → gmac TX
control    wan app ⇄ logical_lan   (DHCP · ARP · bridge · copy fallback)`}
      </CodeBlock>
    </div>
  </Section>
)

const FLAVOR_ROWS = [
  ['h4-production', 'quiet firmware, release SDK, no status endpoint, no test generator, no fault-code LED patterns'],
  ['h4-debug', 'debug SDK, firmware logging, status JSON on port 48125, port-5201 test generator, LED fault codes']
]

export const Performance = () => (
  <Section
    id='performance'
    title='Performance'
    lead='Board-tested h4 build, operator-measured on real hardware with manual iperf3 runs, not lab-certified. Both flavors carry the same datapath tuning.'
  >
    <dl className='flex flex-wrap gap-x-12 gap-y-6'>
      <div>
        <dd className='text-(color:--color-n3-ink) text-[clamp(2.25rem,1.6rem+2vw,3.25rem)] leading-none font-extrabold tracking-[-0.02em] tabular-nums'>
          ~440
        </dd>
        <dt className='text-(color:--color-n3-grey) mt-2 text-[15px]'>Mbps download</dt>
      </div>
      <div>
        <dd className='text-(color:--color-n3-ink) text-[clamp(2.25rem,1.6rem+2vw,3.25rem)] leading-none font-extrabold tracking-[-0.02em] tabular-nums'>
          ~385
        </dd>
        <dt className='text-(color:--color-n3-grey) mt-2 text-[15px]'>Mbps upload</dt>
      </div>
      <div>
        <dd className='text-(color:--color-n3-ink) text-[clamp(2.25rem,1.6rem+2vw,3.25rem)] leading-none font-extrabold tracking-[-0.02em] tabular-nums'>
          1024
        </dd>
        <dt className='text-(color:--color-n3-grey) mt-2 text-[15px]'>2 KiB zero-copy DMA buffers</dt>
      </div>
    </dl>

    <div className='mt-10 overflow-x-auto'>
      <table className='w-full border-collapse text-left'>
        <tbody>
          {FLAVOR_ROWS.map(([name, detail]) => (
            <tr key={name} className='border-t border-(color:--color-n3-line)'>
              <td className='font-mono-plex py-3 pr-6 align-top text-[13px] whitespace-nowrap'>{name}</td>
              <td className='text-(color:--color-n3-grey) py-3 text-[14px] leading-[1.6]'>{detail}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  </Section>
)
