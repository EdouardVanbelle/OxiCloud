#!/bin/bash

if ! which b3sum >/dev/null 2>/dev/null
then
    echo "please install b3sum (brew install b3sum on Mac, apt installb3sum on Debian, etc)" >&2
    exit 1
fi

HASH=""
FILE_CACHE=""

# return the hash of a file (stores into HASH variable)
oxi_hash() {
    if [[ -z "$HASH" || "$FILE_CACHE" != "$1" ]]
    then
        HASH=$(b3sum --no-names "$1")
        FILE_CACHE="$1"
    fi
    echo "$HASH"
}

# returns the local blob localisation
local_blob_path() {
    local BLOB_PREFIX
    oxi_hash "$1" >/dev/null
    BLOB_PREFIX=${HASH:0:2}
    echo ".blobs/$BLOB_PREFIX/$HASH.blob"
}

# returns the preview localisation without it's extension
preview_path() {
    local SIZE
    oxi_hash "$1" >/dev/null
    # default size: icon
    SIZE="${3:-icon}"
    echo ".thumbnails/$SIZE/$HASH"
}

assert_local_blob_existsy() {
    BLOB_PATH=$(local_blob_path "$1")
    STORAGE="$2"
    if [[ -e $STORAGE/$BLOB_PATH ]]
    then
        echo "$BLOB_PATH exists"
        return 0
    else
        echo $'\e[31m'"$BLOB_PATH does not exist"$'\e[0m' >&2
        return 1
    fi
}

assert_preview_existsy() {
    THUMBNAIL_PATH=$(preview_path "$1")
    STORAGE="$2"
    if [[ -e "$STORAGE/$THUMBNAIL_PATH.jpg" || -e "$STORAGE/$THUMBNAIL_PATH.webp" ]]
    then
        echo "thumbnail $THUMBNAIL_PATH.(jpg|webp) exists"
        return 0
    else
        echo $'\e[31m'"thumbnail $THUMBNAIL_PATH.(jpg|webp) does not exist"$'\e[0m' >&2
        echo $STORAGE
        find $STORAGE/.thumbnails
        return 1
    fi
}

