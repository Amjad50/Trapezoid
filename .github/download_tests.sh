#! /bin/bash

set -e

cd test_roms
bash ./downloader.sh ./tests_data.csv
cd ..
