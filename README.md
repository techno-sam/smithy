# Smithy - inspect and modify .mca region files
Smithy is a command line utility to mount Minecraft region (aka Anvil) files as FUSE directories.

## Usage
```sh
smithy mount r.4.2.mca /path/to/mountpoint
```
Files are mounted readonly by default, add the `-w` flag to enable writing.  
Note that Smithy will modify the .mca file in-place, so you may wish to make a backup first.

Each chunk within a region is represented by two files: `x#z#.nbt`, which contains the actual chunk data
and `x#z#.cmp`, which contains the [compression type](https://minecraft.wiki/w/Region_file_format#Payload).
It is essential that a chunk's `.cmp` file is correct, otherwise Minecraft will fail to load that chunk.
Therefore, you should copy the `.cmp` file first when copying a chunk.

> [!NOTE]
> An inspection of Minecraft's code suggests that copying a chunk verbatim should load correctly
> (though it will emit a warning in the logs, and any copied block entities will be broken in exciting ways).

To edit a chunk, you may wish to use Una's fantastic command-line NBT editor, [unbted](https://git.sleeping.town/unascribed/unbted).

### Unmounting
**Do not** simply kill Smithy, as this will not clean up the FUSE connection (unless the `-u` flag is specified).
Instead, use `umount` or `fusermount3 -u` on the mountpoint.

## Installation
Smithy supports linux and (untested) mac os, and inherits [fuser's dependecies](https://github.com/cberner/fuser/blob/master/README.md#dependencies).

```sh
cargo install --git https://github.com/techno-sam/smithy
```

### Completions
Add the following to your shell profile for seamless updates, or pregenerate the completion script to improve shell startup time.
```sh
source <(smithy completion --shell <SHELL>)
```

> [!IMPORTANT]
> Make sure to regenerate the completion script when updating smithy

#### Bash
```sh
CDIR="${XDG_DATA_HOME:-~/.local/share}/bash-completion/completions"
mkdir -vp $CDIR
smithy completion --shell bash --out-dir "$CDIR"
```

#### Fish
```sh
CDIR="${XDG_DATA_HOME:-~/.local/share}/fish/vendor_completions.d"
mkdir -vp $CDIR
smithy completion --shell fish --out-dir "$CDIR"
```

#### Oh My Zsh
```sh
CDIR="~/.oh-my-zsh/custom/completions"
mkdir -vp $CDIR
smithy completion --shell bash --out-dir "$CDIR"
```

#### Other shells
Other shells are also supported, I just don't know what their completion directories are.

## License
```
Smithy
Copyright (C) 2025  Sam Wagenaar

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published
by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
```
