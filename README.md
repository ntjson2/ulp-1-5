# ulp-1-5

## About this project


## Compile from Huff
.\tools\huff-neo.exe contracts\FlashExecutor.huff --bytecode --alt-main FLASH_LOAN_404

## to build folder
$hex = "60538060093d393df360043560243560443573ba12222222228d8ba445958a75a0704d566bf2c95af160643560843563a9059cbb5f52906020529160405260605ff173ba12222222228d8ba445958a75a0704d566bf2c95f5f5f5af1"
$bytes = for ($i = 0; $i -lt $hex.Length; $i += 2) { [Convert]::ToByte($hex.Substring($i, 2), 16) }
[IO.File]::WriteAllBytes("build\flash_executor.bin", $bytes)

Bytecode saved successfully to build\flash_executor.bin using Huff-Neo output!

$hex = "60088060093d393df360605f60605f60f3"
$bytes = for ($i = 0; $i -lt $hex.Length; $i += 2) { [Convert]::ToByte($hex.Substring($i, 2), 16) }
[IO.File]::WriteAllBytes("build\uni_v4_swapper.bin", $bytes)

## Fire up anvil
cmd -> WSL -> ntjson@LAPTOP-74NQ7BNG:~$ anvil