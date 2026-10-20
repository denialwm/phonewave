#!/usr/bin/env python3
"""Reassemble the acquired UFF split ELF without changing signed segments."""
import hashlib,json,os,struct
from pathlib import Path
VAULT=Path(os.environ.get('PHONEWAVE_VAULT','/mnt/development/device-vault'))/'aston'
BUILD=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/aston/fingerprint/build'

def assemble(directory,name):
    if name not in ('uff_spi','uff_gx'):
        raise ValueError('Only the acquired UFF apps are supported')
    header=(directory/(name+'.b00')).read_bytes()
    if len(header)<64 or header[:6]!=b'\x7fELF\x02\x01':
        raise ValueError('Expected little-endian ELF64')
    phoff=struct.unpack_from('<Q',header,32)[0]
    phsize,phnum=struct.unpack_from('<HH',header,54)
    if phsize!=56 or not 1<=phnum<=32 or phoff+phsize*phnum>len(header):
        raise ValueError('Invalid program header table')
    segments=[]
    for i in range(phnum):
        _,_,offset,_,_,size,_,_=struct.unpack_from('<IIQQQQQQ',header,phoff+i*phsize)
        p=directory/f'{name}.b{i:02d}';data=p.read_bytes()
        if len(data)!=size or offset+size>32*1024*1024:
            raise ValueError('Invalid segment extent: '+p.name)
        segments.append((offset,data,p.name))
    output=bytearray(max(o+len(d) for o,d,_ in segments))
    written=bytearray(len(output))
    for offset,data,segment in segments:
        for i,b in enumerate(data,offset):
            if written[i] and output[i]!=b:
                raise ValueError('Conflicting overlapping segment: '+segment)
        output[offset:offset+len(data)]=data
        written[offset:offset+len(data)]=b'\x01'*len(data)
    return bytes(output),{n:hashlib.sha256(d).hexdigest() for _,d,n in segments}

def main():
    out=BUILD;out.mkdir(parents=True,exist_ok=True)
    manifest={}
    for name in ('uff_spi','uff_gx'):
        data,segments=assemble(VAULT/'firmware/factory/odm_b/vendor/firmware',name)
        p=out/(name+'.elf')
        if p.exists() and p.read_bytes()!=data:
            raise ValueError('Refusing to overwrite changed firmware')
        p.write_bytes(data)
        manifest[name]={'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest(),'segments':segments}
    (out/'firmware-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    print(json.dumps({n:{k:v for k,v in m.items() if k!='segments'} for n,m in manifest.items()},indent=2))
if __name__=='__main__':main()
