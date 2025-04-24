# .venv\Scripts\activate
# deactivate
# python txtexport_onefile.py

import os

# Define paths
base_dir = r"c:\Users\Lenovo\Documents\BUSINESS\QUETZAL COLLECTIVE LLC\ULP\ulp-1.5"
prompt_file = os.path.join(base_dir, "prompt_file_list.txt")
output_file = os.path.join(base_dir, "txtexport_flattened", "all_files_combined.txt")

# Ensure output directory exists
os.makedirs(os.path.dirname(output_file), exist_ok=True)

# Read file paths from prompt_file_list.txt
with open(prompt_file, "r") as file:
    file_paths = file.read().strip().splitlines()

# Open the output file for writing with UTF-8 encoding
with open(output_file, "w", encoding="utf-8") as combined_file:
    # Process each file or directory
    for relative_path in file_paths:
        # Construct full source path
        source_path = os.path.join(base_dir, relative_path.lstrip("/"))
        
        # If it's a directory, process all files in the directory
        if os.path.isdir(source_path):
            for root, _, files in os.walk(source_path):
                for file in files:
                    file_path = os.path.join(root, file)
                    relative_file_path = os.path.relpath(file_path, base_dir).replace("\\", "/")
                    # Write file metadata and contents
                    combined_file.write(f"--- START OF FILE: {relative_file_path} ---\n")
                    with open(file_path, "r", encoding="utf-8", errors="ignore") as f:
                        combined_file.write(f.read())
                    combined_file.write(f"\n--- END OF FILE: {relative_file_path} ---\n\n")
        # If it's a file, process it directly
        elif os.path.isfile(source_path):
            combined_file.write(f"--- START OF FILE: {relative_path} ---\n")
            with open(source_path, "r", encoding="utf-8", errors="ignore") as f:
                combined_file.write(f.read())
            combined_file.write(f"\n--- END OF FILE: {relative_path} ---\n\n")
        else:
            print(f"Path not found: {source_path}")

print(f"All files combined into: {output_file}")
