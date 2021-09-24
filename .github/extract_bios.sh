gpg --decrypt --batch --yes --passphrase="$BIOS_PASSPHRASE" ./test_roms/bios.tar.gz.gpg | tar -C ./test_roms -xzv
