#!/bin/bash
#
#
#
RUN_LENGTH=$1
# fix the date/time
NOW=`date +%Y%m%d_%H%M%S`
PCM="/tmp/${RUN_LENGTH}_samp-rate_288000_fu8_audio-48k.raw"
LOGFILE="/tmp/udp_decode_${RUN_LENGTH}sec_${NOW}.log"
#
# This command will not play, just save to PCM
#
DECODE_CMD="timeout ${RUN_LENGTH} target/debug/examples/udp_decode -m 239.192.0.3 -p 5003 --samp-rate 288000 --format u8 --volume 14 --audio-rate 48000 --filename ${PCM}"


echo "time $DECODE_CMD 2>&1" > "$LOGFILE"

{
  printf "%.23s\n" "$(date +'%Y-%m-%dT%H:%M:%S.%N')"
  (time $DECODE_CMD 2>&1)
  printf "%.23s\n" "$(date +'%Y-%m-%dT%H:%M:%S.%N')"
} | nl | tee -a "$LOGFILE"
echo -e "\n ------------------------\n\n"
echo "Output at $LOGFILE"
echo -e "\n"
head ${LOGFILE}
echo -e "\n"
tail ${LOGFILE}
echo "File size of {$PCM}: "`ls -lath ${PCM}`
