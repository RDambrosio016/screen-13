# User Contributions to Screen 13

These subdirectories contain additions, changes, and other things you might find useful while
using _Screen 13_. These user-provided contributions are not guaranteed to work and are untested.

## `.vscode/`

Configuration files for users of _[Visual Studio Code](https://code.visualstudio.com/)_. Copy the
`.vscode/` directory into the root _Screen 13_ project directory in order to enable build and debug
configurations.

**_NOTE:_** Requires installation of the
_[CodeLLDB](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb)_ extension for
debugging.

### `rel-mgmt/`

A script which exercises all test cases and build conditions which must succeed prior to merging new
code into the main branch.

### `screen-13-fx/`

Pre-defined effects and tools built using _Screen 13_ features. Generally anything that requires
shaders or other physical data which shouldn't be part of the main library.