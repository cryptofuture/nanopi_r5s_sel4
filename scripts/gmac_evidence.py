#!/usr/bin/env python3
import hashlib
import json
from pathlib import Path, PurePosixPath
import tarfile

def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()

def safe_import(archive,destination):
    archive=Path(archive);destination=Path(destination)
    if destination.exists():raise ValueError('Evidence destination already exists')
    index={'archive':archive.name,'sha256':digest(archive),'members':[]}
    with tarfile.open(archive) as tar:
        members=tar.getmembers();seen=set();total=0
        for member in members:
            path=PurePosixPath(member.name)
            if (path.is_absolute() or '..' in path.parts or not path.parts or
                '\\' in member.name or not (member.isfile() or member.isdir()) or
                str(path) in seen):raise ValueError('Unsafe or duplicate archive member: '+member.name)
            seen.add(str(path));total+=member.size
            if total>64*1024*1024:raise ValueError('Evidence exceeds 64 MiB bound')
        files={str(PurePosixPath(m.name)) for m in members if m.isfile()}
        for member in members:
            if any(str(parent) in files for parent in PurePosixPath(member.name).parents):
                raise ValueError('File used as an archive directory')
        destination.mkdir(parents=True)
        for member in members:
            target=destination.joinpath(*PurePosixPath(member.name).parts)
            if member.isdir():target.mkdir(parents=True,exist_ok=True)
            else:
                target.parent.mkdir(parents=True,exist_ok=True)
                with tar.extractfile(member) as source,target.open('xb') as output:output.write(source.read())
            index['members'].append({'path':member.name,'type':'directory' if member.isdir() else 'file','size':member.size,'sha256':digest(target) if member.isfile() else None})
    return index

if __name__=='__main__':
    import argparse
    p=argparse.ArgumentParser();p.add_argument('archive',type=Path);p.add_argument('destination',type=Path);p.add_argument('--index',type=Path,required=True);a=p.parse_args()
    a.index.write_text(json.dumps(safe_import(a.archive,a.destination),indent=2)+'\n')
