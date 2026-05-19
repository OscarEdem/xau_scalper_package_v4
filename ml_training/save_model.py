import json
import os

def save_model(name: str, params: dict):
    os.makedirs("models", exist_ok=True)
    file_path = os.path.join("models", f"{name}_config.json")
    with open(file_path, "w") as f:
        json.dump(params, f, indent=4)
    print(f"Saved {file_path}")
