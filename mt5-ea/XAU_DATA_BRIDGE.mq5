// XAU_DATA_BRIDGE.mq5 v4.2
// Data Bridge: Sends market data to the analysis server.
#property strict
#property version   "4.2"


// --- EA Inputs ---
input string ServerUrl = "https://gold-ml-base-server.onrender.com"; // Base URL, endpoints will be appended
input int    NumCloses = 300; // Increased to satisfy server's longest indicator (SMA 200) and provide buffer
input double MaxSpreadPoints = 162;

// --- Strategy Parameters (from best backtest) ---
input int    RsiPeriod = 16;
input int    EmaFastPeriod = 5;
input int    EmaSlowPeriod = 50;
input int    AtrPeriod = 14;
input int    SmaPeriod = 200;
input int    AdxPeriod = 14;                // New: ADX Period for Swing
input double AdxThreshold = 25.0;             // New: ADX Threshold for Swing
input int    ChandelierPeriod = 22;         // New: Chandelier Exit Period
input double ChandelierAtrMult = 3.0;         // New: Chandelier ATR Multiplier
input int    MaxHoldBars = 12;              // New: Max hold bars for scalp time stop

input int    StochKPeriod = 14; // New: Stochastic K Period
input int    StochDPeriod = 3;  // New: Stochastic D Period
input int    StochSlowing = 3;  // New: Stochastic Slowing Period
input double SlAtrMultiplier = 1.0;
input double TpAtrMultiplier = 1.5;

// --- NEW: Kalman Filter Parameters ---
input double KfProcessNoise = 0.01; // q: Process noise for Kalman Filter
input double KfMeasurementNoise = 0.1;  // r: Measurement noise for Kalman Filter

input double DashboardScale = 0.0;            // Scale factor (0.0 = Auto-scale to chart width)

// --- NEW: Real-time Tick Bridge Inputs ---
input bool   EnableTickBridge = true;             // Enable sending real-time ticks via HTTP
input int    TickBridgeInterval = 500;            // Min ms between tick requests to prevent lag
// --- THEME & SIZING HELPERS ---
color THEME_BG      = C'0,0,0';       // Solid Black
color THEME_BORDER  = C'212,175,55';  // Gold accent
color THEME_TEXT    = C'230,230,230';
color THEME_ACCENT  = C'46,204,113';  // Buy (Emerald Green)
color THEME_ACCENT2 = C'231,76,60';   // Sell (Alizarin Red)
color THEME_MUTED   = C'140,140,140';
#define FONT_FACE     "Segoe UI"      // Primary font

// --- Global Variables ---
char post_data[];
char result[];
int g_DashboardX = -1;
int g_DashboardY = -1;
bool g_IsDragging = false;
int g_DragOffsetX = 0;
int g_DragOffsetY = 0;

string g_BaseUrl = ""; // Normalized server URL
// --- Dashboard State Globals ---
string   g_DashStatus = "Initializing...";
datetime g_DashLastBarTime = 0;
ulong    g_DashLastTickTime = 0;
int      g_DashLastResponseCode = 0;
string   g_DashLastTickStatus = "Disabled";

ulong    g_last_data_sent_time = 0; // For flashing indicator

// --- State Globals for Data Processing ---
datetime g_prev_time_m5 = 0;
datetime g_prev_time_m30 = 0;
datetime g_prev_time_h1 = 0;
datetime g_prev_time_h4 = 0;
datetime g_prev_time_d1 = 0;
ulong    g_last_force_sync = 0;

// --- Function Prototypes ---
void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color, const color border_color);
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color, int font_size=10, string font=FONT_FACE);
void UpdateDashboard(string status, datetime last_bar_time, int response_code, string tick_status, ulong last_tick_time);
void CreateDashboardRow(long chart_ID, string key, string value, int x_key, int x_val, int y, color clr_key, color clr_val);
void DrawDashboard();
double GetUiScale();
int ui(int px);
void UpdateDataSendIndicator();
void ProcessData(bool manual_force);

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
int OnInit()
  {
// Make sure the terminal is configured to allow WebRequest
   ChartSetInteger(0, CHART_EVENT_MOUSE_MOVE, true);
// Go to Tools -> Options -> Expert Advisors and add the ServerUrl
   
   // Normalize URL (remove trailing slash if present)
   g_BaseUrl = ServerUrl;
   if(StringSubstr(g_BaseUrl, StringLen(g_BaseUrl)-1) == "/")
      g_BaseUrl = StringSubstr(g_BaseUrl, 0, StringLen(g_BaseUrl)-1);

   Print("XAU Data Bridge initialized. Base URL: ", g_BaseUrl);

   g_DashLastTickStatus = EnableTickBridge ? "Waiting..." : "Disabled";
   UpdateDashboard("Initializing...", 0, 0, g_DashLastTickStatus, 0);

   return(INIT_SUCCEEDED);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void OnDeinit(const int reason)
  {
// Clean up all graphical objects created by this EA
   ObjectsDeleteAll(0, "XauBridge_"); // Prefix for dashboard objects
   ChartRedraw();
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void OnTick()
  {
// --- UI Update for flashing indicator ---
   UpdateDataSendIndicator();

// --- Real-time Tick Bridge Logic ---
   static ulong last_tick_req = 0;
   if(EnableTickBridge && (GetTickCount64() - last_tick_req > (ulong)TickBridgeInterval))
     {
      MqlTick last_tick;
      if(SymbolInfoTick(_Symbol, last_tick))
        {
         // Format tick data as a JSON string
         string tick_json = StringFormat(
                               "{\"symbol\":\"%s\",\"bid\":%.5f,\"ask\":%.5f,\"timestamp\":%llu}",
                               _Symbol,
                               last_tick.bid,
                               last_tick.ask,
                               last_tick.time_msc
                            );
         // Send the tick via a non-blocking HTTP POST request
         char tick_post_data[];
         char tick_result[];
         string tick_headers;
         StringToCharArray(tick_json, tick_post_data);
         last_tick_req = GetTickCount64();
         int tick_res = WebRequest("POST", g_BaseUrl + "/ticks", "Content-Type: application/json", 500, tick_post_data, tick_result, tick_headers);
         g_DashLastTickTime = last_tick.time_msc;
         if(tick_res == 200)
           {
            g_DashLastTickStatus = "OK";
           }
         else
           {
            g_DashLastTickStatus = "Error: " + IntegerToString(tick_res);
           }
        }
     }
// --- Original OnTick Logic (runs on new bar) ---
   static datetime last_bar=0;
   MqlRates rates[];

// Only run the rest of the logic on a new bar
   if(CopyRates(_Symbol, PERIOD_M1, 0, 1, rates) < 1)
      return;
   if(rates[0].time == last_bar)
     {
      return;
     }
   last_bar = rates[0].time;

   ProcessData(false);
  }

//+------------------------------------------------------------------+
//| Process and Send Data                                            |
//+------------------------------------------------------------------+
void ProcessData(bool manual_force)
  {
// --- Pre-trade checks ---
   double ask=SymbolInfoDouble(_Symbol, SYMBOL_ASK);
   double bid=SymbolInfoDouble(_Symbol, SYMBOL_BID);
   double point_val = SymbolInfoDouble(_Symbol, SYMBOL_POINT);
   double spread_raw = ask - bid;
   double spread_pts = spread_raw / point_val;

   PrintFormat("Spread Check: Raw Spread=%.5f, Point=%.5f, Spread Points=%.2f", spread_raw, point_val, spread_pts);

   if(spread_pts > MaxSpreadPoints)
     {
      Print("Spread is too high: ", spread_pts, " points. Skipping.");
      UpdateDashboard("Spread High", TimeCurrent(), 0, g_DashLastTickStatus, g_DashLastTickTime);
      return;
     }

// --- Prepare price data for server ---
   MqlRates m1_rates[], m5_rates[], m30_rates[], h1_rates[], h4_rates[], d1_rates[];
   if(CopyRates(_Symbol, PERIOD_M1, 0, NumCloses, m1_rates) < NumCloses)
     {
      Print("Could not get enough M1 bar data. Need ", NumCloses, " bars.");
      UpdateDashboard("Waiting for M1 data...", TimeCurrent(), 0, g_DashLastTickStatus, g_DashLastTickTime);
      return;
     }

   // --- Optimization: Only fetch and process HTF data if the bar has changed ---
   // Force sync every 60 seconds to handle server restarts/data loss
   bool force_sync = manual_force || (GetTickCount64() - g_last_force_sync > 60000);

   bool update_m5 = (iTime(_Symbol, PERIOD_M5, 0) != g_prev_time_m5) || force_sync;
   bool update_m30 = (iTime(_Symbol, PERIOD_M30, 0) != g_prev_time_m30) || force_sync;
   bool update_h1 = (iTime(_Symbol, PERIOD_H1, 0) != g_prev_time_h1) || force_sync;
   bool update_h4 = (iTime(_Symbol, PERIOD_H4, 0) != g_prev_time_h4) || force_sync;
   bool update_d1 = (iTime(_Symbol, PERIOD_D1, 0) != g_prev_time_d1) || force_sync;

   if(update_m5 && CopyRates(_Symbol, PERIOD_M5, 0, NumCloses, m5_rates) < NumCloses) return;
   if(update_m30 && CopyRates(_Symbol, PERIOD_M30, 0, NumCloses, m30_rates) < NumCloses) return;
   if(update_h1 && CopyRates(_Symbol, PERIOD_H1, 0, NumCloses, h1_rates) < NumCloses) return;
   if(update_h4 && CopyRates(_Symbol, PERIOD_H4, 0, NumCloses, h4_rates) < NumCloses) return;
   if(update_d1 && CopyRates(_Symbol, PERIOD_D1, 0, NumCloses, d1_rates) < NumCloses) return;

   // Update trackers
   if(update_m5 && ArraySize(m5_rates) > 0) g_prev_time_m5 = m5_rates[ArraySize(m5_rates)-1].time;
   if(update_m30 && ArraySize(m30_rates) > 0) g_prev_time_m30 = m30_rates[ArraySize(m30_rates)-1].time;
   if(update_h1 && ArraySize(h1_rates) > 0) g_prev_time_h1 = h1_rates[ArraySize(h1_rates)-1].time;
   if(update_h4 && ArraySize(h4_rates) > 0) g_prev_time_h4 = h4_rates[ArraySize(h4_rates)-1].time;
   if(update_d1 && ArraySize(d1_rates) > 0) g_prev_time_d1 = d1_rates[ArraySize(d1_rates)-1].time;

   if(force_sync) g_last_force_sync = GetTickCount64();

// Build the JSON payload
   string m1_opens_str = "", m1_closes_str = "", m1_highs_str = "", m1_lows_str = "", m1_volumes_str = "";
   string m5_closes_str = "", m5_highs_str = "", m5_lows_str = "", h1_opens_str = "";
   string m30_closes_str = "", h1_closes_str = "", h1_highs_str = "", h1_lows_str = "", h4_closes_str = "", h4_highs_str = "", h4_lows_str = "";
   string d1_opens_str = "", d1_closes_str = "";

   for(int i = 0; i < ArraySize(m1_rates); i++)
     {
      // Use tick_volume as it's generally more available than real_volume
      m1_volumes_str += IntegerToString(m1_rates[i].tick_volume);
      m1_opens_str += DoubleToString(m1_rates[i].open, _Digits);
      m1_closes_str += DoubleToString(m1_rates[i].close, _Digits);
      m1_highs_str += DoubleToString(m1_rates[i].high, _Digits);
      m1_lows_str += DoubleToString(m1_rates[i].low, _Digits);
      if(i < ArraySize(m1_rates) - 1)
        {
         m1_volumes_str += ",";
         m1_opens_str += ",";
         m1_closes_str += ",";
         m1_highs_str += ",";
         m1_lows_str += ",";
        }
     }

   if(update_m5)
     {
      for(int i = 0; i < ArraySize(m5_rates); i++)
        {
         m5_closes_str += DoubleToString(m5_rates[i].close, _Digits);
         m5_highs_str += DoubleToString(m5_rates[i].high, _Digits);
         m5_lows_str += DoubleToString(m5_rates[i].low, _Digits);
         if(i < ArraySize(m5_rates) - 1)
           {
            m5_closes_str += ",";
            m5_highs_str += ",";
            m5_lows_str += ",";
           }
        }
     }

   if(update_m30)
     {
      for(int i = 0; i < ArraySize(m30_rates); i++)
        {
         m30_closes_str += DoubleToString(m30_rates[i].close, _Digits);
         if(i < ArraySize(m30_rates) - 1)
            m30_closes_str += ",";
        }
     }

   if(update_h1)
     {
      for(int i = 0; i < ArraySize(h1_rates); i++)
        {
         h1_closes_str += DoubleToString(h1_rates[i].close, _Digits);
         h1_highs_str += DoubleToString(h1_rates[i].high, _Digits);
         h1_lows_str += DoubleToString(h1_rates[i].low, _Digits);
         h1_opens_str += DoubleToString(h1_rates[i].open, _Digits);
         if(i < ArraySize(h1_rates) - 1)
           {
            h1_closes_str += ",";
            h1_highs_str += ",";
            h1_lows_str += ",";
            h1_opens_str += ",";
           }
        }
     }

   if(update_h4)
     {
      for(int i = 0; i < ArraySize(h4_rates); i++)
        {
         h4_closes_str += DoubleToString(h4_rates[i].close, _Digits);
         h4_highs_str += DoubleToString(h4_rates[i].high, _Digits);
         h4_lows_str += DoubleToString(h4_rates[i].low, _Digits);
         if(i < ArraySize(h4_rates) - 1)
           {
            h4_closes_str += ",";
            h4_highs_str += ",";
            h4_lows_str += ",";
           }
        }
     }

   if(update_d1)
     {
      for(int i = 0; i < ArraySize(d1_rates); i++)
        {
         d1_opens_str += DoubleToString(d1_rates[i].open, _Digits);
         d1_closes_str += DoubleToString(d1_rates[i].close, _Digits);
         if(i < ArraySize(d1_rates) - 1)
           {
            d1_opens_str += ",";
            d1_closes_str += ",";
           }
        }
     }

   long last_m1_timestamp = (long)m1_rates[ArraySize(m1_rates)-1].time;
   long last_m5_timestamp = (ArraySize(m5_rates) > 0) ? (long)m5_rates[ArraySize(m5_rates)-1].time : 0;
   long last_m30_timestamp = (ArraySize(m30_rates) > 0) ? (long)m30_rates[ArraySize(m30_rates)-1].time : 0;
   long last_h1_timestamp = (ArraySize(h1_rates) > 0) ? (long)h1_rates[ArraySize(h1_rates)-1].time : 0;
   long last_h4_timestamp = (ArraySize(h4_rates) > 0) ? (long)h4_rates[ArraySize(h4_rates)-1].time : 0;
   long last_d1_timestamp = (ArraySize(d1_rates) > 0) ? (long)d1_rates[ArraySize(d1_rates)-1].time : 0;

   string json_payload = StringFormat(
                            "{\"symbol\":\"%s\",\"timeframe\":\"M1\","
                            "\"currentPrice\":%.5f,\"spreadPoints\":%.1f,\"priceDecimals\":%d,\"lastM1Timestamp\":%lld,"
                            "\"opens\":[%s],\"closes\":[%s],\"highs\":[%s],\"lows\":[%s],\"volumes\":[%s],"
                            "\"m5Closes\":[%s],\"m5Highs\":[%s],\"m5Lows\":[%s],"
                            "\"m30Closes\":[%s],"
                            "\"h1Closes\":[%s],\"h1Highs\":[%s],\"h1Lows\":[%s],\"h1Opens\":[%s],"
                            "\"h4Closes\":[%s],\"h4Highs\":[%s],\"h4Lows\":[%s],"
                            "\"d1Opens\":[%s],\"d1Closes\":[%s],"
                            "\"lastM5Timestamp\":%lld,\"lastM30Timestamp\":%lld,\"lastH1Timestamp\":%lld,\"lastH4Timestamp\":%lld,\"lastD1Timestamp\":%lld,"
                            "\"rsiPeriod\":%d,\"emaFast\":%d,\"emaSlow\":%d,\"atrPeriod\":%d,\"smaPeriod\":%d,"
                            "\"spreadLimitPoints\":%.1f,"
                            "\"stochKPeriod\":%d,\"stochDPeriod\":%d,\"stochSlowing\":%d,"
                            "\"slAtrMultiplier\":%.2f,\"tpAtrMultiplier\":%.2f,"
                            "\"kfProcessNoise\":%.4f,\"kfMeasurementNoise\":%.4f,"
                            "\"adxPeriod\":%d,\"adxThreshold\":%.1f,"
                            "\"chandelierPeriod\":%d,\"chandelierAtrMult\":%.1f,"
                            "\"maxHoldBars\":%d,"
                            "\"mode\":\"scalp\"}",
                            _Symbol,
                            ask, spread_pts, _Digits, last_m1_timestamp,
                            m1_opens_str, m1_closes_str, m1_highs_str, m1_lows_str, m1_volumes_str,
                            m5_closes_str, m5_highs_str, m5_lows_str, m30_closes_str,
                            h1_closes_str, h1_highs_str, h1_lows_str, h1_opens_str,
                            h4_closes_str, h4_highs_str, h4_lows_str,
                            d1_opens_str, d1_closes_str,
                            last_m5_timestamp, last_m30_timestamp, last_h1_timestamp, last_h4_timestamp, last_d1_timestamp,
                            RsiPeriod, EmaFastPeriod, EmaSlowPeriod, AtrPeriod, SmaPeriod, MaxSpreadPoints,
                            StochKPeriod, StochDPeriod, StochSlowing,
                            SlAtrMultiplier, TpAtrMultiplier,
                            KfProcessNoise, KfMeasurementNoise,
                            AdxPeriod, AdxThreshold, ChandelierPeriod, ChandelierAtrMult, MaxHoldBars
                         );

// --- 1. POST data to the Rust server ---
   ResetLastError();
   string result_headers;
   StringToCharArray(json_payload, post_data);
   int res = WebRequest("POST", g_BaseUrl + "/data", "Content-Type: application/json", 5000, post_data, result, result_headers);
   g_last_data_sent_time = GetTickCount64();
   
   if(res == -1)
     {
      Print("WebRequest failed. Error code: ", GetLastError());
      UpdateDashboard("POST /data Failed", m1_rates[ArraySize(m1_rates)-1].time, res, g_DashLastTickStatus, g_DashLastTickTime);
     }
   else
      if(res != 200)
        {
         Print("Server /data returned non-200 status: ", res);
         Print("Server response: ", CharArrayToString(result));
         UpdateDashboard("Server Error", m1_rates[ArraySize(m1_rates)-1].time, res, g_DashLastTickStatus, g_DashLastTickTime);
        }
      else
        {
         Print("Data POST successful.");
         UpdateDashboard("OK", m1_rates[ArraySize(m1_rates)-1].time, res, g_DashLastTickStatus, g_DashLastTickTime);
        }
  }

//+------------------------------------------------------------------+
//| UI Scale Helper                                                  |
//+------------------------------------------------------------------+
double GetUiScale()
  {
   if(DashboardScale > 0.0)
      return DashboardScale;
   long w = ChartGetInteger(ChartID(), CHART_WIDTH_IN_PIXELS);
// Base width 1440px = scale 1.0
   double s = (double)w / 1440.0;
   if(s < 0.75)
      s = 0.75;
   if(s > 1.5)
      s = 1.5;
   return s;
  }

int ui(int px) { return (int)MathRound(px * GetUiScale()); }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color)
  {
   if(ObjectFind(chart_ID, name) < 0)
     {
      ObjectCreate(chart_ID, name, OBJ_RECTANGLE_LABEL, 0, 0, 0);
      ObjectSetInteger(chart_ID, name, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID, name, OBJPROP_SELECTABLE, false);
      ObjectSetInteger(chart_ID, name, OBJPROP_BACK, true);
     }
   ObjectSetInteger(chart_ID, name, OBJPROP_XDISTANCE, x);
   ObjectSetInteger(chart_ID, name, OBJPROP_YDISTANCE, y);
   ObjectSetInteger(chart_ID, name, OBJPROP_XSIZE, width);
   ObjectSetInteger(chart_ID, name, OBJPROP_YSIZE, height);
   ObjectSetInteger(chart_ID, name, OBJPROP_BGCOLOR, (long)bg_color);
  }
//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color, const color border_color)
  {
// Alternative approach: Create a border with a larger rectangle behind a smaller one.
// This avoids BORDER_STYLE_ROUND_EDGES which may not be supported on older MT5 builds.
   string border_name = name + "_Border";
   int border_thickness = 2;

// 1. Create the outer rectangle (the border)
   if(ObjectFind(chart_ID, border_name) < 0)
     {
      ObjectCreate(chart_ID, border_name, OBJ_RECTANGLE_LABEL, 0, 0, 0);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_SELECTABLE, false);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_BACK, true);
     }
   ObjectSetInteger(chart_ID, border_name, OBJPROP_XDISTANCE, x);
   ObjectSetInteger(chart_ID, border_name, OBJPROP_YDISTANCE, y);
   ObjectSetInteger(chart_ID, border_name, OBJPROP_XSIZE, width);
   ObjectSetInteger(chart_ID, border_name, OBJPROP_YSIZE, height);
   ObjectSetInteger(chart_ID, border_name, OBJPROP_BGCOLOR, (long)border_color);

// 2. Create the inner rectangle (the background) on top of the border
   CreatePanel(chart_ID, name, x + border_thickness, y + border_thickness, width - (border_thickness * 2), height - (border_thickness * 2), bg_color); // Call the overloaded function
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color, int font_size=10, string font=FONT_FACE)
  {
   if(ObjectFind(chart_ID, name) < 0)
     {
      ObjectCreate(chart_ID, name, OBJ_LABEL, 0, 0, 0);
      ObjectSetInteger(chart_ID, name, OBJPROP_CORNER, (long)CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID, name, OBJPROP_SELECTABLE, false);
      ObjectSetInteger(chart_ID, name, OBJPROP_BACK, false);
      ObjectSetInteger(chart_ID, name, OBJPROP_ZORDER, 101);
     }
   ObjectSetInteger(chart_ID, name, OBJPROP_XDISTANCE, x);
   ObjectSetInteger(chart_ID, name, OBJPROP_YDISTANCE, y);
   ObjectSetString(chart_ID, name, OBJPROP_TEXT, text);
   ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, text_color);
   ObjectSetString(chart_ID, name, OBJPROP_FONT, font);
   ObjectSetInteger(chart_ID, name, OBJPROP_FONTSIZE, font_size);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateDashboardRow(long chart_ID, string key, string value, int x_key, int x_val, int y, color clr_key, color clr_val)
  {
   string key_name = "XauBridge_" + key;
   string val_name = "XauBridge_" + key + "_Val";

// Create the label for the key (e.g., "RSI")
   CreateLabel(chart_ID, key_name, x_key, y, key, clr_key, ui(9));

// Create the label for the value (e.g., "55.2")
   CreateLabel(chart_ID, val_name, x_val, y, value, clr_val, ui(9));
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void UpdateDashboard(string status, datetime last_bar_time, int response_code, string tick_status, ulong last_tick_time)
  {
   g_DashStatus = status;
   if(last_bar_time > 0) g_DashLastBarTime = last_bar_time;
   g_DashLastResponseCode = response_code;
   g_DashLastTickStatus = tick_status;
   if(last_tick_time > 0) g_DashLastTickTime = last_tick_time;

   DrawDashboard();
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void DrawDashboard()
  {
   long chart_ID = ChartID();
// Panel sizing
   int panelW = ui(240);
   int panelH = ui(150);

   if(g_DashboardX == -1)
      g_DashboardX = ui(10);
   if(g_DashboardY == -1)
      g_DashboardY = ui(10);
   int x_pos = g_DashboardX;
   int y_pos = g_DashboardY;

   string panelBorder = "XauBridge_Panel_Border";
   if(ObjectFind(chart_ID, panelBorder) < 0)
     {
      ObjectCreate(chart_ID, panelBorder, OBJ_RECTANGLE_LABEL, 0, 0, 0);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_SELECTABLE, false);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_BACK, false);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_ZORDER, 100);
     }
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_XDISTANCE, x_pos);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_YDISTANCE, y_pos);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_XSIZE, panelW);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_YSIZE, panelH);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_BGCOLOR, (long)THEME_BG);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_BORDER_COLOR, (long)THEME_BORDER);

   CreateLabel(chart_ID, "XauBridge_Title", x_pos + ui(10), y_pos + ui(8), "XAU DATA BRIDGE", THEME_BORDER, ui(10), "Segoe UI Semibold");

   // --- ADD INDICATOR DOT ---
   string indicator_name = "XauBridge_SendIndicator";
   // Use a large dot character ● (U+25CF)
   CreateLabel(chart_ID, indicator_name, x_pos + panelW - ui(25), y_pos + ui(10), "●", THEME_MUTED, ui(12));

   int left_x = x_pos + ui(10);
   int right_x = x_pos + ui(120);
   int cur_y = y_pos + ui(30);

   // Status
   color status_color = THEME_TEXT;
   if(g_DashStatus == "OK") status_color = THEME_ACCENT;
   else if(StringFind(g_DashStatus, "Error") >= 0 || StringFind(g_DashStatus, "Failed") >= 0 || StringFind(g_DashStatus, "High") >= 0) status_color = THEME_ACCENT2;
   CreateDashboardRow(chart_ID, "Status", g_DashStatus, left_x, right_x, cur_y, THEME_MUTED, status_color);
   cur_y += ui(18);

   // Last Bar Sent
   string bar_time_str = (g_DashLastBarTime > 0) ? TimeToString(g_DashLastBarTime, TIME_SECONDS) : "---";
   CreateDashboardRow(chart_ID, "Last Bar Sent", bar_time_str, left_x, right_x, cur_y, THEME_MUTED, THEME_TEXT);
   cur_y += ui(18);

   // Last Tick Sent
   string tick_status_str = EnableTickBridge ? g_DashLastTickStatus : "Disabled";
   color tick_status_color = THEME_TEXT;
   if(tick_status_str == "OK") tick_status_color = THEME_ACCENT;
   else if(StringFind(tick_status_str, "Error") >= 0) tick_status_color = THEME_ACCENT2;
   CreateDashboardRow(chart_ID, "Tick Status", tick_status_str, left_x, right_x, cur_y, THEME_MUTED, tick_status_color);
   cur_y += ui(18);

   // Server Response Code
   string response_str = (g_DashLastResponseCode != 0) ? IntegerToString(g_DashLastResponseCode) : "---";
   color response_color = (g_DashLastResponseCode == 200) ? THEME_ACCENT : (g_DashLastResponseCode == 0 ? THEME_TEXT : THEME_ACCENT2);
   CreateDashboardRow(chart_ID, "HTTP Response", response_str, left_x, right_x, cur_y, THEME_MUTED, response_color);
   cur_y += ui(25);

   // Force Sync Button
   string btn = "XauBridge_ForceSync";
   if(ObjectFind(chart_ID, btn) < 0) {
      ObjectCreate(chart_ID, btn, OBJ_BUTTON, 0, 0, 0);
      ObjectSetInteger(chart_ID, btn, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetString(chart_ID, btn, OBJPROP_TEXT, "Force Sync");
      ObjectSetInteger(chart_ID, btn, OBJPROP_ZORDER, 101);
      ObjectSetInteger(chart_ID, btn, OBJPROP_BGCOLOR, C'60,60,60');
      ObjectSetInteger(chart_ID, btn, OBJPROP_COLOR, C'255,255,255');
   }
   ObjectSetInteger(chart_ID, btn, OBJPROP_XDISTANCE, left_x);
   ObjectSetInteger(chart_ID, btn, OBJPROP_YDISTANCE, cur_y);
   ObjectSetInteger(chart_ID, btn, OBJPROP_XSIZE, ui(100));
   ObjectSetInteger(chart_ID, btn, OBJPROP_YSIZE, ui(22));
   ObjectSetInteger(chart_ID, btn, OBJPROP_FONTSIZE, ui(9));

   // Cleanup old objects that are no longer used
   ObjectsDeleteAll(0, "XauBridge_Arrow_");
   ObjectsDeleteAll(0, "XauBridge_SrvFVG_");
   ObjectsDeleteAll(0, "XauBridge_SrvLiq_");
   ObjectsDeleteAll(0, "XauBridge_FVG_");
   ObjectsDeleteAll(0, "XauBridge_Liq_");
   ObjectDelete(chart_ID, "XauBridge_Compact_Text");
   ObjectDelete(chart_ID, "XauBridge_Signal");
   ObjectDelete(chart_ID, "XauBridge_Reason");
   ObjectDelete(chart_ID, "XauBridge_Reason2");
   ObjectDelete(chart_ID, "XauBridge_Balance");
   ObjectDelete(chart_ID, "XauBridge_Balance_Val");
   ObjectDelete(chart_ID, "XauBridge_Spread");
   ObjectDelete(chart_ID, "XauBridge_Spread_Val");
   ObjectDelete(chart_ID, "XauBridge_Current P/L");
   ObjectDelete(chart_ID, "XauBridge_Current P/L_Val");
   ObjectDelete(chart_ID, "XauBridge_Separator");
   ObjectsDeleteAll(chart_ID, "XauBridge_Conv_");
   ObjectDelete(chart_ID, "XauBridge_CloseAll");
   ObjectDelete(chart_ID, "XauBridge_ZoneToggle");
   ObjectDelete(chart_ID, "XauBridge_CollapseToggle");

   ChartRedraw(chart_ID);
  }

//+------------------------------------------------------------------+
//| Chart Event Handler                                              |
//+------------------------------------------------------------------+
void OnChartEvent(const int id, const long &lparam, const double &dparam, const string &sparam)
  {
   if(id == CHARTEVENT_MOUSE_MOVE)
     {
      int x = (int)lparam;
      int y = (int)dparam;
      int mouse_state = (int)StringToInteger(sparam);

      if(mouse_state == 1) // Left button down
        {
         if(!g_IsDragging)
           {
            string name = "XauBridge_Panel_Border";
            if(ObjectFind(0, name) >= 0)
              {
               long px = ObjectGetInteger(0, name, OBJPROP_XDISTANCE);
               long py = ObjectGetInteger(0, name, OBJPROP_YDISTANCE);
               long pw = ObjectGetInteger(0, name, OBJPROP_XSIZE);
               long ph = ObjectGetInteger(0, name, OBJPROP_YSIZE);

               if(x >= px && x <= px + pw && y >= py && y <= py + ph)
                 {
                  g_IsDragging = true;
                  g_DragOffsetX = x - (int)px;
                  g_DragOffsetY = y - (int)py;
                  ChartSetInteger(0, CHART_MOUSE_SCROLL, false);
                 }
              }
           }
         else
           {
            g_DashboardX = x - g_DragOffsetX;
            g_DashboardY = y - g_DragOffsetY;
            if(g_DashboardX < 0)
               g_DashboardX = 0;
            if(g_DashboardY < 0)
               g_DashboardY = 0;
            DrawDashboard();
            ChartRedraw();
           }
        }
      else
         if(g_IsDragging)
           {
            g_IsDragging = false;
            ChartSetInteger(0, CHART_MOUSE_SCROLL, true);
           }
     }
   else
      if(id == CHARTEVENT_OBJECT_CLICK)
        {
         if(sparam == "XauBridge_ForceSync")
           {
            ObjectSetInteger(0, "XauBridge_ForceSync", OBJPROP_STATE, false);
            ProcessData(true);
           }
        }
      else
         if(id == CHARTEVENT_CHART_CHANGE)
           {
            DrawDashboard();
           }
  }

//+------------------------------------------------------------------+
//| Update Data Send Indicator color for flashing effect             |
//+------------------------------------------------------------------+
void UpdateDataSendIndicator()
  {
   string name = "XauBridge_SendIndicator";
   if(ObjectFind(0, name) < 0)
      return; // Dashboard not drawn yet

   color clr = THEME_MUTED;
   // Flash for 1.5 seconds after a send attempt
   if(GetTickCount64() - g_last_data_sent_time < 1500)
     {
      clr = THEME_ACCENT;
     }
   ObjectSetInteger(0, name, OBJPROP_COLOR, clr);
  }
//+------------------------------------------------------------------+
