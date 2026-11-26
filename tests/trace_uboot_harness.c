#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>

typedef unsigned char u8;
typedef unsigned long ulong;
#define BIT(n) (1UL << (n))
#define UCLASS_MMC 1
#define PART_TYPE_EFI 5
#define FS_TYPE_FAT 1
#define CMD_RET_FAILURE 1
#define R5S_GET_SP(value) ((value) = 0x50000000UL)
#define R5S_MASK_INTERRUPTS() ((void)0)
#define R5S_BARRIER() ((void)0)
#define DECLARE_GLOBAL_DATA_PTR
struct cmd_tbl { int unused; };
struct udevice { int unused; };
struct mmc { int unused; };
struct blk_desc { unsigned long blksz, lba; int devnum; };
struct disk_partition { unsigned long start, size; };
struct global_data { unsigned long start_addr_sp, relocaddr, ram_top; void *fdt_blob; };
static struct global_data state = {0x50000000, 0x50000000, 0xf0000000, (void *)0x70000000};
static struct global_data *gd = &state;
ulong mem_malloc_start = 0x60000000, mem_malloc_end = 0x62000000;
static struct udevice device;
static struct mmc card;
static struct blk_desc disk = {512, 122138624, 1};
static unsigned scenario, expected_error, observed_error, stages[16], stage_count, waits[32], wait_count;
static unsigned read_count, handed_off, stops, gpio_state;
static jmp_buf finished;

static unsigned current_el(void) { return scenario == 1 ? 1 : 2; }
static unsigned fdt_totalsize(void *blob) { return 4096; }
static void writel(unsigned value, void *address) {
    if ((uintptr_t)address == 0xfe770004) {
        assert(value == 0x04000000 || value == 0x04000400);
        gpio_state = value == 0x04000400;
    }
}
static unsigned readl(void *address) { return 0; }
static void clrbits_le32(void *address, unsigned bits) {}
static void dsb(void) {}
static void isb(void) {}
static void flush_dcache_all(void) {}
static void dcache_disable(void) {}
static void icache_disable(void) {}
static unsigned long get_sctlr(void) { return scenario == 8 ? BIT(0) : 0; }
static void uclass_first_device(int class, struct udevice **dev) { *dev = scenario == 2 ? NULL : &device; }
static void uclass_next_device(struct udevice **dev) { *dev = NULL; }
static unsigned long dev_read_addr(struct udevice *dev) { return 0xfe2b0000; }
static struct mmc *mmc_get_mmc_dev(struct udevice *dev) { return &card; }
static int mmc_init(struct mmc *mmc) { return 0; }
#define IS_SD(mmc) true
static struct blk_desc *mmc_get_blk_desc(struct mmc *mmc) { return &disk; }
static int part_get_info_by_type(struct blk_desc *d, int part, int type, struct disk_partition *p)
{
    assert(part == 1 && type == PART_TYPE_EFI);
    p->start = 32768; p->size = 131072;
    return scenario == 3;
}
static int fs_set_blk_dev(const char *type, const char *part, int fs)
{
    assert(strcmp(type, "mmc") == 0 && strcmp(part, "1:1") == 0);
    return 0;
}
static int fs_size(const char *name, loff_t *size) { assert(!strcmp(name, "loader.img")); *size = scenario == 4 ? 43 : 42; return 0; }
static int fs_read(const char *name, ulong address, loff_t off, loff_t len, loff_t *actual)
{
    assert(address == 0x30000000 && off == 0 && len == 42);
    read_count++; *actual = len; return 0;
}
static void sha256_csum_wd(const u8 *p, loff_t n, u8 *digest, int chunk) { memset(digest, scenario == 5 ? 1 : 0, 32); }
static void mdelay(unsigned ms)
{
    assert(wait_count < 32); waits[wait_count++] = ms;
    if (ms == 2000) {
        unsigned error = waits[0] == 1500;
        unsigned code = (wait_count - (error ? 4 : 2) - 1) / 2;
        unsigned prefix = error ? 4 : 2;
        assert(waits[0] == (error ? 1500 : 1000) && waits[1] == 500);
        if (error) assert(waits[2] == 1500 && waits[3] == 500);
        for (unsigned i = prefix; i + 1 < wait_count - 1; i += 2)
            assert(waits[i] == 150 && waits[i + 1] == 250);
        assert(gpio_state == 0);
        if (error) { observed_error = code; longjmp(finished, 1); }
        assert(stage_count < 16); stages[stage_count++] = code;
        wait_count = 0;
        if (R5S_STOP_CHECKPOINT == code && ++stops == 2) longjmp(finished, 1);
    }
}
#define U_BOOT_CMD(name, maximum, repeat, command, usage, help) \
    static int (*registered)(struct cmd_tbl *, int, int, char *const[]) = command
#include "../firmware/r5sboot-trace.c"
void r5s_enter(unsigned long entry)
{
    assert(entry == 0x30000000); handed_off++;
    if (scenario == 9) r5s_returned();
    longjmp(finished, 1);
}
int main(int argc, char **argv)
{
    scenario = argc > 1 ? atoi(argv[1]) : 0;
    expected_error = scenario == 8 ? 6 : scenario == 9 ? 7 : scenario;
    if (scenario == 6) mem_malloc_start = 0x20000000;
    if (!setjmp(finished)) { registered(NULL, 0, 0, NULL); assert(!"command returned"); }
    if (R5S_STOP_CHECKPOINT == 3 && scenario == 0) {
        assert(stage_count == 4 && stages[0] == 1 && stages[1] == 2 && stages[2] == 3 && stages[3] == 3);
        assert(handed_off == 0);
    } else if (scenario) {
        assert(observed_error == expected_error);
        assert(stages[0] == 1);
        assert(read_count == (scenario == 5 || scenario == 8 || scenario == 9));
        assert(handed_off == (scenario == 9));
    } else {
        assert(stage_count == 3 && stages[0] == 1 && stages[1] == 2 && stages[2] == 3);
        assert(handed_off == 1);
    }
    return 0;
}
