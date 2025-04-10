# ulp-1-5

## Compile from Huff


Here’s a summary of the last few issues and how they were resolved:

❌ Error: stream did not contain valid UTF-8

Fix: You tried reading a compiled .bin file with read_to_string, which expects UTF-8. Replaced it with std::fs::read to load raw bytes.

❌ Error: Invalid character 'ï' at position 0

Fix: This was caused by a BOM (Byte Order Mark) or saving the .bin file with incorrect encoding. Ensured output was saved using Out-File -Encoding ASCII or > in a shell that doesn’t prepend BOMs.

❌ Error: No connection could be made because the target machine actively refused it

Fix: Anvil (the local Ethereum testnet) wasn’t running. Started Anvil with anvil in WSL to enable RPC at http://localhost:8545.

❌ Error: EVM error StackUnderflow

Fix: Tried to deploy raw logic bytecode without a constructor. Recompiled with the -f (constructor wrapper) flag using:

powershell
Copy
Edit
.\tools\huff.exe -f contracts\FlashExecutor.huff FLASH_LOAN_404 > build\flash_executor.bin
✅ Final success:

After the above, cargo run successfully deployed FlashExecutor with no EVM errors and printed the deployed address.