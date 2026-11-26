#include <assert.h>
#include <string.h>
#include "ring.h"
static _Alignas(64) struct gmac_desc desc[GMAC_SLOTS];
static _Alignas(64) unsigned char buffers[GMAC_SLOTS*GMAC_BUFFER];
static unsigned fences;
static void barrier(void) {fences++;}
int main(void) {
    struct gmac_ring ring;unsigned char packet[1514],out[1514];memset(packet,0x5a,sizeof(packet));
    assert(gmac_ring_init(&ring,desc,buffers,0x22000001,barrier,barrier)<0);
    assert(gmac_ring_init(&ring,desc,buffers,UINT64_C(0x100000000),barrier,barrier)<0);
    assert(gmac_ring_init(&ring,desc,buffers,0x22000000,barrier,barrier)==0);
    for (unsigned round=0;round<100;round++) {
        for (unsigned i=0;i<GMAC_SLOTS;i++) assert(gmac_tx_submit(&ring,packet,sizeof(packet))==1514);
        assert(gmac_tx_submit(&ring,packet,60)==-1);assert(gmac_tx_reclaim(&ring)==0);
        for (unsigned i=0;i<GMAC_SLOTS;i++) desc[i].word[3]=0;
        for (unsigned i=0;i<GMAC_SLOTS;i++) assert(gmac_tx_reclaim(&ring)==1);
        assert(gmac_tx_reclaim(&ring)==0);
    }
    assert(gmac_tx_submit(&ring,packet,14)==60);
    for (unsigned i=14;i<GMAC_BUFFER;i++) assert(buffers[i]==0);
    desc[0].word[3]=0x8000;assert(gmac_tx_reclaim(&ring)==-2);
    gmac_rx_prepare(&ring);assert(gmac_rx_copy(&ring,out,sizeof(out),0)==0);
    for (unsigned iteration=0;iteration<1000;iteration++) {
        unsigned i=ring.consumer;memcpy(buffers+i*GMAC_BUFFER,packet,1514);
        desc[i].word[3]=0x30000000|1514;
        assert(gmac_rx_copy(&ring,out,sizeof(out),0)==1514);assert(!memcmp(out,packet,1514));
        assert(desc[i].word[3]==(GMAC_OWN|0x1000000));
    }
    unsigned bad[]={0x30000000|32767,0x30008000|100,0x70000000|100,0x10000000|100,0x30000000|13};
    for (unsigned n=0;n<sizeof(bad)/sizeof(*bad);n++) {desc[ring.consumer].word[3]=bad[n];assert(gmac_rx_copy(&ring,out,sizeof(out),0)<0);}
    desc[ring.consumer].word[3]=0x30000000|1518;assert(gmac_rx_copy(&ring,out,sizeof(out),1)==1514);
    desc[ring.consumer].word[3]=0x30000000|1514;assert(gmac_rx_copy(&ring,out,10,0)<0);
    assert(fences>6000);return 0;
}
