# .venv\Scripts\activate
# deactivate
# python txtexport.py

import os
import shutil

# Define paths
base_dir = r"c:\Users\Lenovo\Documents\BUSINESS\QUETZAL COLLECTIVE LLC\ULP\ulp-1.5"
prompt_file = os.path.join(base_dir, "prompt_file_list.txt")
output_dir = os.path.join(base_dir, "txtexport_flattened")

# Ensure output directory exists
os.makedirs(output_dir, exist_ok=True)

# Read file paths from prompt_file_list.txt
with open(prompt_file, "r") as file:
    file_paths = file.read().strip().splitlines()

# List to store processed files for project_file_system.txt
processed_files = []

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
                processed_files.append(f"/{relative_file_path}")
                # Copy and rename the file
                destination_path = os.path.join(output_dir, f"{os.path.basename(file_path)}.txt")
                shutil.copyfile(file_path, destination_path)
                print(f"Copied and renamed: {file_path} -> {destination_path}")
    # If it's a file, process it directly
    elif os.path.isfile(source_path):
        processed_files.append(relative_path)
        # Copy and rename the file
        destination_path = os.path.join(output_dir, f"{os.path.basename(source_path)}.txt")
        shutil.copyfile(source_path, destination_path)
        print(f"Copied and renamed: {source_path} -> {destination_path}")
    else:
        print(f"Path not found: {source_path}")

# Write processed files to project_file_system.txt
project_file_system_path = os.path.join(output_dir, "project_file_system.txt")
with open(project_file_system_path, "w", encoding="utf-8") as project_file:
    project_file.write("**ULP 1.5 Project Files**\n\n")
    project_file.write("\n".join(processed_files))
    print(f"Exported processed files to: {project_file_system_path}")
