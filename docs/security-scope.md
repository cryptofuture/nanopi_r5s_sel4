# Scope and trust

This is a standalone boot/platform prototype, not a firewall or a production
security product. It makes no formal verification, malicious-DMA isolation,
throughput, secure boot or watchdog recovery claim.

The first Rust PD receives one uncached GPIO4 page, no eMMC/SD/PCIe regions,
network capabilities or architectural timer IRQ. Shared clock/reset/pinmux
registers are prepared by trusted SD firmware, not mapped into Rust. GPIO4 is
a whole bank page, so this trusted prototype PD can affect other bank pins.
The only intended writes are SYS GPIO4_D2 using split high-half masks.

U-Boot networking/PCIe/USB support is disabled. eMMC and SPI controllers are
disabled in its device tree. SD reads use FIFO in SPL and U-Boot; IDMAC and SD
interrupts are disabled before entry. There are no initialized network or PCIe
DMA drivers to hand off. Opaque BootROM/DDR/BL31 behavior cannot be proven from
their public interfaces; validate that trust assumption on hardware before
claiming stronger isolation. Secondary cores stay firmware-parked.

The kernel uses upstream RK3568 GICv3 and architectural timer support. ITS/MSI-X,
NICs, NAT, DHCP, rules, VMs, a console and watchdogs are deferred. The Rust pattern
uses bounded busy loops without timer register access; timing is uncalibrated
and the PD consumes its CPU budget. Syscall/runtime panic diagnostics may use
the debug kernel's serial facility, but successful LED operation does not wait
for a host, UART input or a network service.

Image and payload SHA-256 detect accidental corruption. A malicious replacement
firmware or manifest is not authenticated by this project. No secure-boot/OTP
settings are changed. The existing FIT's signature metadata is not interpreted
as proof of the board's secure-boot policy.
