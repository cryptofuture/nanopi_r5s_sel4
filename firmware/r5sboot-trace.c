#include <command.h>
#include <dm.h>
#include <dm/read.h>
#include <fs.h>
#include <mmc.h>
#include <part.h>
#include <malloc.h>
#include <linux/libfdt.h>
#include <asm/io.h>
#include <asm/system.h>
#include <asm/global_data.h>
#include <cpu_func.h>
#include <u-boot/sha256.h>
#include <linux/delay.h>
#include "r5s_payload.h"
#define R5S_WAIT(ms) mdelay(ms)
#define R5S_WRITE(v, a) writel(v, (void *)(a))
#include "r5s_trace.h"

DECLARE_GLOBAL_DATA_PTR;
#define LOAD 0x30000000UL
#ifndef R5S_GET_SP
#define R5S_GET_SP(value) __asm__ volatile("mov %0, sp" : "=r"(value))
#endif
#ifndef R5S_MASK_INTERRUPTS
#define R5S_MASK_INTERRUPTS() __asm__ volatile("msr daifset, #0xf" ::: "memory")
#endif
void r5s_enter(unsigned long entry);
void r5s_returned(void);

static int error(unsigned int code, const char *reason)
{
    printf("R5S error %u: %s\n", code, reason);
    r5s_error(code);
    return CMD_RET_FAILURE;
}

void r5s_returned(void)
{
    error(7, "loader unexpectedly returned; signaling only if CPU state is still usable");
}

static bool intersects_work(unsigned long address, unsigned long size)
{
    return size && (address + size < address ||
                   (address < 0x40000000UL && address + size > 0x20000000UL));
}

static int do_r5sboot(struct cmd_tbl *cmdtp, int flag, int argc, char *const argv[])
{
    struct udevice *dev;
    struct mmc *card = NULL;
    struct blk_desc *disk = NULL;
    struct disk_partition partition;
    loff_t size, actual;
    u8 digest[32];
    static const u8 expected[32] = R5S_PAYLOAD_SHA256;
    char part[24];
    unsigned long sp;

    writel(0x03000000, (void *)0xfdd2037cUL);
    writel(0x00030000, (void *)0xfdd20458UL);
    writel(0x0f000000, (void *)0xfdc60078UL);
    writel(0x04000000, (void *)0xfe770004UL);
    writel(0x04000400, (void *)0xfe77000cUL);
    r5s_checkpoint(1);
    printf("R5S CurrentEL=%u\n", current_el());
    if (current_el() != 2)
        return error(1, "EL2 required; no attempted elevation");
    R5S_GET_SP(sp);
    if (gd->start_addr_sp < 0x41000000UL || gd->relocaddr < 0x40000000UL ||
        gd->ram_top < gd->relocaddr || sp < gd->start_addr_sp - 0x1000000UL ||
        intersects_work(mem_malloc_start, mem_malloc_end - mem_malloc_start) ||
        intersects_work((unsigned long)gd->fdt_blob, fdt_totalsize(gd->fdt_blob)))
        return error(6, "live U-Boot stack/heap/DT reservations intersect working RAM");
    printf("R5S live: reloc=%lx sp=%lx stack_top=%lx heap=[%lx,%lx) dt=%p size=%x ram_top=%llx\n",
           gd->relocaddr, sp, gd->start_addr_sp, mem_malloc_start, mem_malloc_end,
           gd->fdt_blob, fdt_totalsize(gd->fdt_blob), (unsigned long long)gd->ram_top);
    for (uclass_first_device(UCLASS_MMC, &dev); dev; uclass_next_device(&dev)) {
        if (dev_read_addr(dev) != 0xfe2b0000UL)
            continue;
        card = mmc_get_mmc_dev(dev);
        if (!card || mmc_init(card) || !IS_SD(card))
            return error(2, "intended SD initialization/type failed");
        disk = mmc_get_blk_desc(card);
        break;
    }
    if (!disk)
        return error(2, "SD controller not found");
    if (part_get_info_by_type(disk, 1, PART_TYPE_EFI, &partition))
        return error(3, "GPT partition 1 unavailable");
    if (disk->blksz != 512 || partition.start != 32768 || partition.size != 131072 ||
        disk->lba < partition.start + partition.size)
        return error(4, "partition/capacity geometry differs");
    snprintf(part, sizeof(part), "%d:1", disk->devnum);
    if (fs_set_blk_dev("mmc", part, FS_TYPE_FAT) || fs_size("loader.img", &size))
        return error(3, "FAT/loader.img unavailable");
    if (size <= 0 || size != R5S_PAYLOAD_SIZE || size > 0x04000000 || LOAD + size > 0x40000000UL)
        return error(4, "payload length/load range invalid");
    if (fs_set_blk_dev("mmc", part, FS_TYPE_FAT) || fs_read("loader.img", LOAD, 0, size, &actual))
        return error(3, "payload read failed");
    if (actual != size)
        return error(4, "short payload read");
    sha256_csum_wd((const u8 *)LOAD, size, digest, 0);
    if (memcmp(digest, expected, sizeof(digest)))
        return error(5, "payload SHA-256 mismatch");
    r5s_checkpoint(2);
    if (current_el() != 2)
        return error(1, "EL changed before handoff");
    if ((readl((void *)0xfe2b0000UL) & (BIT(25) | BIT(5))) ||
        (readl((void *)0xfe2b0080UL) & BIT(7)))
        return error(6, "SD DMA unexpectedly active despite FIFO policy");
    r5s_checkpoint(3);
    clrbits_le32((void *)0xfe2b0000UL, BIT(25) | BIT(5));
    clrbits_le32((void *)0xfe2b0080UL, BIT(7));
    writel(0, (void *)0xfe2b0024UL);
    writel(0xffffffff, (void *)0xfe2b0044UL);
    if (readl((void *)0xfe2b0024UL))
        return error(6, "SD interrupt mask did not clear");
    R5S_MASK_INTERRUPTS();
    flush_dcache_all();
    dcache_disable();
    icache_disable();
    dsb(); isb();
    if (get_sctlr() & (BIT(0) | BIT(2) | BIT(12)))
        return error(6, "MMU/cache state did not clear");
    r5s_enter(LOAD);
    return error(7, "handoff helper returned");
}
U_BOOT_CMD(r5sboot, 1, 0, do_r5sboot, "trace the native R5S Microkit handoff", "");
