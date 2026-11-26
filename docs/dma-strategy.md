# DMA strategy qualification for the pinned R5S runtime

Result: the cacheable-Normal + seL4 VSpace maintenance API is **source/configuration qualified**, not hardware-tested. End-to-end native DMA remains **blocked**, and no raw-I/O image is shipped. No new mapping ABI, kernel modification, MAIR write, cache disable or SDK replacement is needed for the preferred option. The link image is independent of this work.

## Preferred: cached Normal memory with authorized maintenance

The pinned Microkit builder (`tool/microkit/src/capdl/builder.rs`, PD_VSPACE_CAP_IDX=3 and insertion near line 706) explicitly grants each PD its own VSpace for cache cleaning. This is not an assumed page capability or an arbitrary API copied from general docs. The actual built CapDL report contains `cnode_gmac` slot 3 -> `pud_gmac`, through the PD's root CNode. Generated SDK bindings include `seL4_ARM_VSpace_Clean_Data`, `Invalidate_Data` and `CleanInvalidate_Data`.

The unchanged kernel is seL4 62e345c240a15671484530b4e6e069ce266c28d7, ARM_HYPERVISOR_SUPPORT=1, MAX_NODES=1, L1_CACHE_LINE_SIZE_BITS=6, ARM_SMMU disabled. CONFIG_AARCH64_USER_CACHE_ENABLE=1 is also present, with native SCTLR UCI/UCT setup; this does not grant arbitrary cache instructions such as EL0 DC IVAC. The selected cap-based maintenance route avoids assuming a framework's direct-instruction permissions. `makeUserPagePTE` in `src/arch/arm/64/kernel/vspace.c:797` selects S2_NORMAL=15 (inner/outer write-back) for cached mappings and S2_DEVICE_nGnRnE=0 for cached=false. Native HCR includes HCR_DC/TGE/VM (`include/arch/arm/armv/armv8-a/64/armv/vcpu.h`), establishing the native default-cacheable first-stage regime; there is no guest Linux or independently programmed guest page table. Kernel direct RAM mappings are Normal write-back. Device mappings retain Device attributes. This is an audit of compiled configuration and source constructors, not a dump of live PTEs.

The VSpace flush decoder checks service type, ASID, user range, mapped page, **one-page boundaries**, and, under hypervisor support, the translated physical range against the kernel RAM window. It then uses the kernel's Normal direct alias to perform maintenance, including architectural and platform cache operations/barriers. A nonresident mapping can return success without performing maintenance, so future DMA qualification MUST separately prove the pool's mappings/physical allocation; syscall success alone is insufficient.

`drivers/gmac0/cache.c` is compiled for AArch64 against these actual generated headers. It invokes cap 3, splits requests at 4 KiB boundaries, requires complete 64-byte lines within an explicit page-aligned pool, checks every invocation result, stops on failure, and brackets transfers with DSB. Host tests inject success/failure and cross-page requests. It is deliberately **not linked into the link image** and no cache syscall was executed on the R5S in this task. Actual invocation results and cache visibility remain hardware-untested. Object inspection also exposes the expected C `__sel4_ipc_buffer` dependency. The Rust runtime exports its IPC-buffer object rather than that C pointer; no unreviewed alias shim was added. A Rust-native binding or an audited C IPC-buffer adapter with exclusive access is still required before linking this helper into the Rust driver. The cap/API qualification does not claim that FFI integration is complete.

For a future raw image, reserve low physical RAM via the actual Microkit allocator, not an arbitrary scratch address; verify source/copy/kernel/PD/PT/stack/reservation non-overlap in the resulting CapDL layout. Record CPU VA, PA and bus addresses separately. All CPU aliases must remain Normal write-back; no cached/Device or cached/NC alias of the pool. Loader mappings must no longer be live at handoff, kernel direct alias attributes must agree, and monitor/probe must not have writable DMA mappings. A bounded copied protocol queue avoids sharing hardware-owned packet cache lines with the parser.

Ownership protocol: CPU owns a TX buffer/descriptor, clears padding, writes data then cleans payload to PoC; writes descriptor fields, cleans them, sets OWN and cleans its whole exclusive descriptor line; DSB before tail notification. Before CPU checks completion/RX, invalidate only complete exclusive lines, acquire ordering after observing OWN clear, validate flags/lengths, then invalidate/read payload. Before returning RX ownership, clean any CPU-dirtied buffer lines and descriptor contents, then OWN with ordering and clean. Never clean stale CPU copies of another hardware-owned descriptor. Metadata and unrelated CPU variables cannot share those lines.

The existing preparatory `ring.c` uses 16-byte contiguous descriptors. Four independent ownership domains share a 64-byte cache line. It was suitable only under its explicitly stated verified Normal-NC contract; it is **not admissible unchanged for cached DMA**. A future cached adaptation needs an exclusive cache-line stride, audited hardware DSL/AXI-width configuration, cache-aware publication/reclamation, cache operation error handling and full-ring wrap tests. Reference Rockchip U-Boot uses a 64-bit AXI width; that and device address width/translation still need an explicit target qualification in the native backend, not a virtual-pointer cast.

## Other options

A scoped Normal Non-Cacheable extension is possible in principle: stage-2 has a Normal NC encoding, but the existing user attribute currently exposes only cacheable versus Device. Such an extension would need a dedicated RAM-only attribute/capability path, matching tool/libsel4/kernel headers, all translation-level/alias checks and a separately named no-Ethernet regression image. Globally changing cached=false would break MMIO and is rejected. No such extension is implemented because the supported cached API is smaller.

A framework/sDDF strategy could reduce future driver work only after checking its exact cached/uncached assumptions, cap slots, cache operations, descriptor ownership and RK3568 register compatibility. Published DWMAC support is not proof of drop-in compatibility. No new framework dependency or incompatible SDK binary was adopted.

## Remaining DMA gates

`dma-map.json` has no allocated area or invented bus address. The capture supplies no runtime MAC feature dump or effective DMA-remapping proof. Root-level absence of dma-ranges/iommus/dma-coherent is not, by itself, proof of identity translation, coherency or confinement. A source-complete low-address bus/width audit, allocated pool, cache-line-exclusive descriptor backend, actual MAC/MTL/DMA setup and bounded driver/probe queue integration remain required. These are not reasons to hold the completed link-only image.

If a targeted register capture is needed for the native feature audit after link testing/recovery, the operator can run the Linux driver's advertised read-only register dump, saving it on eMMC:

```sh
ethtool -d eth0 > /root/r5s-flash/gmac-bringup/eth0-register-dump.txt
```

Do not install tools, change link settings or use /dev/mem automatically. That output can identify MAC features; it cannot prove DMA translation or isolation. No broad inventory rerun is requested.

DMA is trusted. ARM_SMMU is disabled in this SDK and no other effective GMAC DMA firewall is demonstrated. CPU capabilities, Rust safety and lack of PCIe are not DMA confinement. Host mocks, object compilation and source-qualified APIs do not establish packet I/O. Only a later controlled fresh external challenge/response/ACK and retained peer report can establish native RX/TX.

The component compile can be repeated on the prepared development checkout:

```sh
. scripts/env
aarch64-none-elf-gcc -std=gnu11 -O2 -ffreestanding -Wall -Wextra -Werror -I out/native-net/work/microkit/release/microkit-sdk-2.4.0/board/nanopi_r5s_trace/debug/include -c drivers/gmac0/cache.c -o out/gmac-bringup/objects/cache.o
```
