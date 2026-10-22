#!/bin/bash

# Set EZT_COMPILER to the path of the `compile` binary if it is not on PATH.
export SML="${EZT_COMPILER:-compile}"

USER=$1      # 1001
SRC_FILE=$2  # example.sm
WRKSPC=$3    # e.g. /tmp/eztrading/codesrv
TEMPLATE=$4  # e.g. ./package

ROOT=$WRKSPC/$USER

cp -r $TEMPLATE/* $ROOT/ || exit -1
cd $ROOT                 || exit -1

BAK=".bak"
if [[ $OSTYPE == 'linux'* ]]; then
    BAK=""
fi

sed -i $BAK "s/path_to_the_main_sm_file/${SRC_FILE//\//\\/}/g" Makefile || exit -1
sed -i $BAK "s/path_to_scheduler_src/${ROOT//\//\\/}\\/common\\/scheduler.cpp/g" Makefile || exit -1

make clean || exit -1
make || exit -1
make scheduler || exit -1
# if [[ $OSTYPE == 'darwin'* ]]; then
#     cp ${SRC_FILE%.sm}.dylib $ROOT/
# elif [[ $OSTYPE == 'linux'* ]]; then
#     cp ${SRC_FILE%.sm}.so $ROOT/
# else
#     echo "OS $OSTYPE is not supported"
# fi
