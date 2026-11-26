import struct

def cells(value):
    if len(value)%4:raise ValueError('Partial DT cell')
    return list(struct.unpack('>'+str(len(value)//4)+'I',value))
def text(value):return value.rstrip(b'\0').decode()
def number(value):
    result=0
    for v in value:result=(result<<32)|v
    return result

class Fdt:
    def __init__(self,data):
        if len(data)<40:raise ValueError('Short FDT')
        magic,total,off,strings,reserve,version,compatible,cpu,string_size,struct_size=struct.unpack_from('>10I',data)
        if magic!=0xd00dfeed or total>len(data) or version<17 or compatible>17:raise ValueError('Invalid FDT header')
        if off+struct_size>total or strings+string_size>total or reserve<40:raise ValueError('Invalid FDT bounds')
        self.reservations=[]
        while True:
            if reserve+16>total:raise ValueError('Unterminated reservation table')
            address,size=struct.unpack_from('>QQ',data,reserve);reserve+=16
            if not address and not size:break
            self.reservations.append([address,address+size])
        table=data[strings:strings+string_size];end=off+struct_size;stack=[];self.nodes={};finished=False
        while off+4<=end:
            token=struct.unpack_from('>I',data,off)[0];off+=4
            if token==1:
                nul=data.find(b'\0',off,end)
                if nul<0:raise ValueError('Invalid node name')
                name=data[off:nul].decode();off=(nul+4)&~3
                if '/' in name or name in ('.','..'):raise ValueError('Invalid node name')
                stack.append(name);path='/'+ '/'.join(stack[1:])
                if path in self.nodes:raise ValueError('Duplicate node')
                self.nodes[path]={}
            elif token==2:
                if not stack:raise ValueError('Unbalanced DT')
                stack.pop()
            elif token==3:
                if not stack or off+8>end:raise ValueError('Invalid DT property')
                size,nameoff=struct.unpack_from('>II',data,off);off+=8
                nul=table.find(b'\0',nameoff)
                if nul<0 or off+size>end:raise ValueError('Invalid property bounds')
                name=table[nameoff:nul].decode();path='/'+ '/'.join(stack[1:])
                if name in self.nodes[path]:raise ValueError('Duplicate property')
                self.nodes[path][name]=data[off:off+size];off=(off+size+3)&~3
            elif token==4:continue
            elif token==9:
                if stack:raise ValueError('Unclosed DT node')
                finished=True;break
            else:raise ValueError('Invalid DT token')
        if not finished:raise ValueError('Missing DT end')
        self.handles={}
        for path,props in self.nodes.items():
            if 'phandle' in props and 'linux,phandle' in props and props['phandle']!=props['linux,phandle']:raise ValueError('Conflicting node phandles')
            for name in ('phandle','linux,phandle'):
                if name in props:
                    value=cells(props[name])
                    if len(value)!=1 or value[0] in (0,0xffffffff):raise ValueError('Invalid phandle')
                    if value[0] in self.handles and self.handles[value[0]]!=path:raise ValueError('Duplicate phandle')
                    self.handles[value[0]]=path
    def resolve(self,handle):
        if handle not in self.handles:raise ValueError('Unresolved phandle')
        return self.handles[handle]
    def refs(self,path,prop,provider_cells):
        values=cells(self.nodes[path][prop]);result=[]
        while values:
            node=self.resolve(values.pop(0));count=cells(self.nodes[node][provider_cells])[0]
            if len(values)<count:raise ValueError('Truncated provider arguments')
            result.append({'node':node,'args':values[:count]});values=values[count:]
        return result
    def regs(self,path):
        parent=path.rsplit('/',1)[0] or '/';props=self.nodes[parent]
        ac=cells(props.get('#address-cells',b'\0\0\0\2'))[0];sc=cells(props.get('#size-cells',b'\0\0\0\1'))[0]
        values=cells(self.nodes[path]['reg'])
        if not ac+sc or len(values)%(ac+sc):raise ValueError('Bad reg cells')
        return [[number(values[i:i+ac]),number(values[i+ac:i+ac+sc])] for i in range(0,len(values),ac+sc)]
