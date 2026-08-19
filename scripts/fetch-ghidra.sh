#!/usr/bin/env bash
# fetch-ghidra.sh — OPTIONAL dev-time reverse-engineering tooling.
#
# Downloads Ghidra 12.1.2 + Temurin JDK 21 into DecryptProject/ and slims
# them to the ARM-Cortex headless-analysis subset used when working on the
# firmware animation pipeline (dispatcher/renderer hook RE).
#
# Nothing here is required to build or run Cloudy AF: the app never calls
# Ghidra at runtime (RE results ship as JSON descriptors), and
# DecryptProject/ is gitignored, so the main repo stays small.
#
# Usage:  scripts/fetch-ghidra.sh
# Result: DecryptProject/ghidra_12.1.2_PUBLIC/  (~125 MB)
#         DecryptProject/tools/jdk-21/          (~210 MB)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/DecryptProject"
mkdir -p "$DEST"
cd "$DEST"

GHIDRA_URL="https://github.com/NationalSecurityAgency/ghidra/releases/download/Ghidra_12.1.2_build/ghidra_12.1.2_PUBLIC_20260605.zip"
JDK_URL="https://api.adoptium.net/v3/binary/latest/21/ga/linux/x64/jdk/hotspot/normal/eclipse"

if [ ! -d ghidra_12.1.2_PUBLIC ]; then
    echo ">> downloading Ghidra 12.1.2"
    curl -fL --retry 3 --retry-delay 5 -o ghidra.zip "$GHIDRA_URL"
    unzip -q ghidra.zip
    rm ghidra.zip
fi

if [ ! -d tools/jdk-21 ]; then
    echo ">> downloading Temurin JDK 21"
    mkdir -p tools
    curl -fL --retry 3 --retry-delay 5 -o tools/jdk21.tar.gz "$JDK_URL"
    tar -xzf tools/jdk21.tar.gz -C tools
    mv tools/jdk-21* tools/jdk-21
    rm tools/jdk21.tar.gz
fi

echo ">> slimming to ARM headless-analysis subset"
cd ghidra_12.1.2_PUBLIC
rm -rf docs Extensions GPL Ghidra/Debug
rm -rf Ghidra/Features/FunctionID Ghidra/Features/BSim \
       Ghidra/Features/GhidraServer Ghidra/Features/PyGhidra \
       Ghidra/Features/VersionTracking Ghidra/Features/GraphServices \
       Ghidra/Features/PDB Ghidra/Features/Sarif \
       Ghidra/Features/SystemEmulation Ghidra/Features/MicrosoftDmang \
       Ghidra/Features/MicrosoftCodeAnalyzer Ghidra/Features/CodeCompare \
       Ghidra/Features/ProgramDiff Ghidra/Features/FunctionGraph \
       Ghidra/Features/BytePatterns
rm -rf Ghidra/Features/Base/data/typeinfo Ghidra/Features/Base/data/symbols
find Ghidra -name '*-src.zip' -delete
cd Ghidra/Processors
ls | grep -vE '^(ARM|DATA)$' | xargs rm -rf
cd ../../..
rm -rf tools/jdk-21/jmods tools/jdk-21/lib/src.zip

du -sh ghidra_12.1.2_PUBLIC tools/jdk-21
cat <<EOF

Done. Headless usage:
  JAVA_HOME="$DEST/tools/jdk-21" \\
    $DEST/ghidra_12.1.2_PUBLIC/support/analyzeHeadless <projdir> <proj> \\
    -import <firmware.bin> -processor ARM:LE:32:Cortex -loader-baseAddr 0x0 \\
    -scriptPath $DEST/ghidra-scripts \\
    -preScript HeadlessAFPre.java -postScript HeadlessAFCount.java
EOF
