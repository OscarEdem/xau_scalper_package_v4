import torch
import torch.nn as nn
from torch.utils.data import Dataset, DataLoader
from dataset import load_all_timeframes
import numpy as np
import os
import json

SEQ_LEN = 50
BATCH_SIZE = 32
EPOCHS = 20
LR = 0.001
VALIDATION_SPLIT = 0.2 # Use 20% of the data for validation
TIMEFRAMES = ["m5", "m30", "h1", "h4", "d1"]

# ---- Dataset ----
class OHLCVDataset(Dataset):
    def __init__(self, close_prices, scaler, seq_len=SEQ_LEN):
        self.data = close_prices
        self.scaler = scaler
        self.seq_len = seq_len

    def __len__(self):
        return len(self.data) - self.seq_len

    def __getitem__(self, idx):
        # Input sequence and target are both from the raw close prices
        x_raw = self.data[idx : idx + self.seq_len]
        y_raw = self.data[idx + self.seq_len] # Target is the *next* price
        # Normalize both using the same scaler
        x_norm = (x_raw - self.scaler['mean']) / self.scaler['std']
        y_norm = (y_raw - self.scaler['mean']) / self.scaler['std']
        return torch.from_numpy(x_norm).float().unsqueeze(-1), torch.tensor(y_norm, dtype=torch.float32)

# ---- Model ----
class LSTMModel(nn.Module):
    def __init__(self, input_size=1, hidden_size=64, num_layers=2):
        super().__init__()
        self.lstm = nn.LSTM(input_size, hidden_size, num_layers, batch_first=True)
        self.fc = nn.Linear(hidden_size, 1)

    def forward(self, x):
        out, _ = self.lstm(x)
        out = self.fc(out[:, -1, :]) # Get output of the last time step
        return out # No activation, as we predict a normalized price

# ---- Training ----
os.makedirs("models", exist_ok=True)
data_all = load_all_timeframes()

for tf, df in data_all.items():
    close_prices = df["ohlcv"][:, 3] # Index 3 is 'close'

    # Split data into training and validation sets
    split_idx = int(len(close_prices) * (1 - VALIDATION_SPLIT))
    train_prices = close_prices[:split_idx]
    val_prices = close_prices[split_idx:]

    # 1. Calculate and save scaler parameters
    scaler_params = {
        "mean": float(np.mean(train_prices)),
        "std": float(np.std(train_prices))
    }
    scaler_path = f"models/lstm_{tf}_scaler.json"
    with open(scaler_path, "w") as f:
        json.dump(scaler_params, f, indent=4)
    print(f"Saved scaler to {scaler_path}")

    # 2. Create Dataset and DataLoader
    train_dataset = OHLCVDataset(train_prices, scaler_params)
    train_loader = DataLoader(train_dataset, batch_size=BATCH_SIZE, shuffle=True)
    val_dataset = OHLCVDataset(val_prices, scaler_params)
    val_loader = DataLoader(val_dataset, batch_size=BATCH_SIZE, shuffle=False)
    
    model = LSTMModel()
    optimizer = torch.optim.Adam(model.parameters(), lr=LR)
    criterion = nn.MSELoss()
    
    for epoch in range(EPOCHS):
        # --- Training Loop ---
        model.train()
        train_loss = 0.0
        for x_batch, y_batch in train_loader:
            optimizer.zero_grad()
            y_pred = model(x_batch)
            loss = criterion(y_pred.squeeze(), y_batch)
            loss.backward()
            optimizer.step()
            train_loss += loss.item()
        
        # --- Validation Loop ---
        model.eval()
        val_loss = 0.0
        with torch.no_grad():
            for x_batch, y_batch in val_loader:
                y_pred = model(x_batch)
                loss = criterion(y_pred.squeeze(), y_batch)
                val_loss += loss.item()

        avg_train_loss = train_loss / len(train_loader)
        avg_val_loss = val_loss / len(val_loader)
        print(f"{tf} Epoch {epoch+1}/{EPOCHS} Train Loss: {avg_train_loss:.5f}, Val Loss: {avg_val_loss:.5f}")
    
    # 3. Export to ONNX
    model.eval() # Set model to evaluation mode
    onnx_path = f"models/lstm_{tf}.onnx"
    
    # Create a dummy input with the correct shape [batch, sequence, features]
    dummy_input = torch.randn(1, SEQ_LEN, 1, requires_grad=True)

    torch.onnx.export(model,
                      (dummy_input,), # The input needs to be a tuple
                      onnx_path,
                      dynamo=False,              # Use the stable 'script' exporter
                      export_params=True,        # Store the trained weights in the model file
                      opset_version=17,          # Use a stable and widely supported opset version
                      do_constant_folding=True,
                      input_names=['input'],   # The name Rust expects
                      output_names=['output'], # The name Rust expects
                      dynamic_axes={'input': {0: 'batch_size'}, 'output': {0: 'batch_size'}})
    print(f"Exported LSTM model for {tf} to {onnx_path}")
