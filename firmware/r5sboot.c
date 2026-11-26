// SPDX-License-Identifier: GPL-2.0-or-later
#include <command.h>
#include <asm/io.h>
#include <linux/delay.h>
#ifndef R5S_UBOOT_DIAGNOSTIC
#include <dm.h>
#include <dm/read.h>
#include <fs.h>
#include <mmc.h>
#include <asm/system.h>
#include <asm/global_data.h>
#include <cpu_func.h>
#include <u-boot/sha256.h>
#include "r5s_payload.h"

#define LOAD 0x30000000UL
DECLARE_GLOBAL_DATA_PTR;
#endif

#define GPIO ((void __iomem *)0xfe770000UL)

static void led_setup(void)
{
	writel(0x03000000, (void *)0xfdd2037cUL);
	writel(0x00030000, (void *)0xfdd20458UL);
	writel(0x0f000000, (void *)0xfdc60078UL);
	writel(0x04000000, GPIO + 4);
	writel(0x04000400, GPIO + 12);
}

static void led(bool on)
{
	writel(0x04000000 | (on ? 0x400 : 0), GPIO + 4);
	dsb();
}

#ifndef R5S_UBOOT_DIAGNOSTIC
static int fail(void)
{
	led(true);
	puts("R5S: payload refused; power off and remove SD\n");
	for (;;)
		asm volatile("wfe");
	return CMD_RET_FAILURE;
}
#endif

static int do_r5sboot(struct cmd_tbl *cmdtp, int flag, int argc,
		     char *const argv[])
{
#ifdef R5S_UBOOT_DIAGNOSTIC
	led_setup();
	led(false);
	mdelay(1000);
	for (;;) {
		led(true);
		mdelay(750);
		led(false);
		mdelay(250);
		led(true);
		mdelay(750);
		led(false);
		mdelay(2250);
	}
#else
	struct udevice *dev;
	struct mmc *card;
	loff_t size, actual;
	u8 digest[32];
	static const u8 expected[32] = R5S_PAYLOAD_SHA256;
	char part[24];

	/* CRU GPIO4 gates/reset and GRF GPIO4_D2 mux, verified against pinned sources. */
	led_setup();
	led(true);
	mdelay(1000);
	led(false);
	if (current_el() != 2)
		return fail();
	if (gd->start_addr_sp < 0x41000000UL ||
	    gd->relocaddr < 0x40000000UL || gd->ram_top < gd->relocaddr)
		return fail();
	for (uclass_first_device(UCLASS_MMC, &dev); dev;
	     uclass_next_device(&dev)) {
		if (dev_read_addr(dev) != 0xfe2b0000UL)
			continue;
		card = mmc_get_mmc_dev(dev);
		if (!card || mmc_init(card) || !IS_SD(card))
			return fail();
		snprintf(part, sizeof(part), "%d:1", mmc_get_blk_desc(card)->devnum);
		if (fs_set_blk_dev("mmc", part, FS_TYPE_FAT) ||
		    fs_size("loader.img", &size) || size != R5S_PAYLOAD_SIZE ||
		    size <= 0 || size > 0x04000000)
			return fail();
		if (fs_set_blk_dev("mmc", part, FS_TYPE_FAT) ||
		    fs_read("loader.img", LOAD, 0, size, &actual) || actual != size)
			return fail();
		sha256_csum_wd((const u8 *)LOAD, size, digest, 0);
		if (memcmp(digest, expected, sizeof(digest)))
			return fail();
		/* FIFO reads use no DMA; also disable the SD IDMAC before handoff. */
		clrbits_le32((void *)0xfe2b0000UL, BIT(25) | BIT(5));
		clrbits_le32((void *)0xfe2b0080UL, BIT(7));
		writel(0, (void *)0xfe2b0024UL);
		asm volatile("msr daifset, #0xf" ::: "memory");
		flush_dcache_all();
		dcache_disable();
		icache_disable();
		dsb();
		isb();
		((void (*)(void))LOAD)();
		return fail();
	}
	return fail();
#endif
}

#ifdef R5S_UBOOT_DIAGNOSTIC
U_BOOT_CMD(r5sboot, 1, 0, do_r5sboot, "repeat the diagnostic SYS LED pattern", "");
#else
U_BOOT_CMD(r5sboot, 1, 0, do_r5sboot, "boot the pinned R5S SD payload", "");
#endif
