def dma_area(area,occupied):
    required={'cpu_virtual','physical','device','size','alignment','attributes','owner','provenance','bus_identity_verified'}
    if set(area)!=required:raise ValueError('Incomplete DMA provenance')
    v,p,b,n,a=(area[k] for k in ('cpu_virtual','physical','device','size','alignment'))
    if (any(type(x)!=int for x in (v,p,b,n,a)) or n<=0 or a<64 or a&(a-1) or
        v<0 or p<0x20000000 or p+n>0x40000000 or b<0 or b+n>0x100000000 or
        any(x%a for x in (v,p,b)) or area['attributes']!='Normal-NC' or
        not area['owner'] or not area['provenance'] or area['bus_identity_verified'] is not True):
        raise ValueError('Unqualified DMA area')
    for start,end in occupied:
        if p<end and start<p+n:raise ValueError('DMA overlap')
    return p,p+n
