

CC=c++
CFLAGS=--std=c++20 -g3 -O0
CARGO=cargo
CLANG=clang
CLANGFLAGS=-dynamiclib --std=c++20 -lstdc++
# Path to the SML compiler (the `compile` binary built from core/transpiler)
SML?=compile
LIBS=
HEADERDIR=inc
RUSTFLAGS=-Awarnings
SMLLIBS=libs
SMLFLAGS=-I $(SMLLIBS)
SMLFILES=$(shell find . -name '*.sm')
SCHEDULER=path_to_scheduler_src
SMLFILE=path_to_the_main_sm_file

BUILDDIR=.
SOURCEDIR=common
SOURCES := $(shell find $(SOURCEDIR) -name '*.cpp')
COMMON_OBJECTS := $(addprefix $(BUILDDIR)/,$(SOURCES:%.cpp=%.o))

SOURCES += $(SMLFILES:%.sm=%.cpp)
LIBRARY=$(SMLFILE:%.sm=%.dylib)
SCH_LIB=$(SCHEDULER:%.cpp=%.dylib)
UNAME_S := $(shell uname -s)

ifeq ($(UNAME_S),Linux)
	CFLAGS+=-fPIC
	CLANGFLAGS=-shared -ldl -fPIC -std=c++20 -lstdc++ -pthread
	LIBS=-ldl
	LIBRARY=$(SMLFILE:%.sm=%.so)
	SCH_LIB=$(SCHEDULER:%.cpp=%.so)
endif

# Get list of object files, with paths
OBJECTS := $(addprefix $(BUILDDIR)/,$(SOURCES:%.cpp=%.o))

.PHONY: all sml clean

all: $(LIBRARY)

$(LIBRARY): sml $(OBJECTS)
	$(CLANG) $(CLANGFLAGS) $(OBJECTS) -o $(LIBRARY)

$(BUILDDIR)/%.o: %.cpp
	$(CC) $(CFLAGS) -I$(HEADERDIR) -I. -c $< -o $@

sml: $(SMLFILE)
	$(SML) $(SMLFLAGS) $<
	$(CC) $(CFLAGS) -I$(HEADERDIR) -I. -c $(<:%.sm=%.cpp) -o $(<:%.sm=%.o)

scheduler: $(COMMON_OBJECTS)
	$(CC) $(CFLAGS) -DSCHEDULER_INTERFACE -I$(HEADERDIR) -I. -c $(SCHEDULER) -o $(SCHEDULER:%.cpp=%.o) $(LIBS)
	$(CLANG) $(CLANGFLAGS) $(COMMON_OBJECTS) -o $(SCH_LIB) $(LIBS)

%.sm:
	$(SML) $<

clean:
	rm -f $(LIBRARY) $(OBJECTS) $(SMLFILES:%.sm=%.cpp) $(SMLFILES:%.sm=%.h) $(SMLFILES:%.sm=%.smo) $(SCH_LIB)

distclean: clean