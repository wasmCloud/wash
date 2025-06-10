# Blobstore Filesystem

This component is a wash plugin that exports `wasi:blobstore/blobstore@0.2.0-draft`, `wasi:blobstore/container@0.2.0-draft`, and `wasi:blobstore/types@0.2.0-draft` and implements the blobstore functions in terms of `wasi:filesystem` APIs. This is a reference implementation for how you can use a component plugin to extend the functionality of `wash dev`.

## Requirements

This component must be passed a directory as a `wasi:filesystem/preopen` with read and write permissions. This directory will be used as the root "container" for all blobstore operations.
