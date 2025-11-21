// XAU_ScalperBridge.mq5 v4.2 - Merged and Corrected
// Combines advanced UI with robust indicator handling.
#property strict
#property version   "4.2"
#include <Trade\Trade.mqh>


// --- EA Inputs ---
input string ServerUrl = "https://major-scalper-v4.onrender.com"; // Base URL, endpoints will be appended
input double RiskPercent = 0.5;
input int    NumCloses = 300; // Increased to satisfy server's longest indicator (SMA 200) and provide buffer
input double MaxSpreadPoints = 165;
input ulong  MagicNumber = 1337;

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

// --- Pyramiding Inputs ---
input bool   EnablePyramiding = true;         // Enable adding to winning positions
input int    MaxPyramidEntries = 3;           // Maximum number of simultaneous entries
input double PyramidProfitPips = 15.0;        // Pips in profit required before adding a position
input double PyramidRiskScale = 0.5;          // Scale risk for next entry (e.g., 0.5 = 50% of previous risk)

// --- Logging Inputs ---
input bool   EnableServerLogging = true;

// --- Global Variables ---
CTrade trade;
char post_data[];
char result[];


// --- NEW: Global variables for backtester-style trailing stop ---
long   g_trade_ticket = 0;       // Ticket of the currently managed trade
double g_high_since_entry = 0.0; // Highest high since the long trade was opened
double g_low_since_entry = 0.0;  // Lowest low since the short trade was opened


// --- Function Prototypes ---
void ClosePositions(ENUM_POSITION_TYPE direction);
void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color, const color border_color);
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color);
void UpdateDashboard(string action, string reason, string tp, string sl, double current_spread, double current_balance, double current_pl, double next_lot_size);
double CalculateLotSize(double stop_loss_pips, string symbol, double point_value, double risk_percentage_override);
string GetJsonValue(string json, string key, bool is_string);
string GetNestedJsonValue(string json, string object_key, string value_key, bool is_string);
int CountOpenPositions(ENUM_POSITION_TYPE direction);
void LogEvent(string event_type, ulong ticket, string symbol, string direction, double lot_size, double price, double sl, double tp, double profit, string comment);

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
int OnInit()
  {
// Make sure the terminal is configured to allow WebRequest
   trade.SetExpertMagicNumber(MagicNumber);
// Go to Tools -> Options -> Expert Advisors and add the ServerUrl
   Print("XAU Scalper Bridge initialized. Server URL: ", ServerUrl);

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

// --- Pre-trade checks ---
   double ask=SymbolInfoDouble(_Symbol, SYMBOL_ASK);
   double bid=SymbolInfoDouble(_Symbol, SYMBOL_BID);
   double point_val = SymbolInfoDouble(_Symbol, SYMBOL_POINT);
   double spread_raw = ask - bid;
   double spread_pts = spread_raw / point_val;

// --- Calculate current P/L for any open position ---
   double current_pl = 0.0;
   for(int i = PositionsTotal() - 1; i >= 0; i--)
     {
      if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber)
        {
         current_pl = PositionGetDouble(POSITION_PROFIT);
         // We found our position, no need to loop further
         break;
        }
     }

   PrintFormat("Spread Check: Raw Spread=%.5f, Point=%.5f, Spread Points=%.2f", spread_raw, point_val, spread_pts);

   if(spread_pts > MaxSpreadPoints)
     {
      Print("Spread is too high: ", spread_pts, " points. Skipping.");
      // Still update dashboard to show high spread
      UpdateDashboard("hold", "Spread too high", "-", "-", spread_pts, AccountInfoDouble(ACCOUNT_BALANCE), current_pl, 0.0);
      return;
     }

// --- Prepare price data for server ---
   MqlRates m1_rates[], m5_rates[], m30_rates[], h1_rates[], h4_rates[], d1_rates[];
   if(CopyRates(_Symbol, PERIOD_M1, 0, NumCloses, m1_rates) < NumCloses)
     {
      Print("Could not get enough M1 bar data. Need ", NumCloses, " bars.");
      return;
     }
   if(CopyRates(_Symbol, PERIOD_M5, 0, NumCloses, m5_rates) < NumCloses)
     {
      Print("Could not get enough M5 bar data. Need ", NumCloses, " bars.");
      return;
     }
   if(CopyRates(_Symbol, PERIOD_M30, 0, NumCloses, m30_rates) < NumCloses)
     {
      Print("Could not get enough M30 bar data. Need ", NumCloses, " bars.");
      return;
     }
   if(CopyRates(_Symbol, PERIOD_H1, 0, NumCloses, h1_rates) < NumCloses)
     {
      Print("Could not get enough H1 bar data. Need ", NumCloses, " bars.");
      return;
     }
   if(CopyRates(_Symbol, PERIOD_H4, 0, NumCloses, h4_rates) < NumCloses)
     {
      Print("Could not get enough H4 bar data. Need ", NumCloses, " bars.");
      return;
     }
   if(CopyRates(_Symbol, PERIOD_D1, 0, NumCloses, d1_rates) < NumCloses)
     {
      Print("Could not get enough D1 bar data. Need ", NumCloses, " bars.");
      return;
     }

// --- Prepare open positions data for server ---
   string open_positions_json = "[";
   int open_pos_count = 0;
   for(int i = PositionsTotal() - 1; i >= 0; i--)
     {
      if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber)
        {
         if(open_pos_count > 0)
            open_positions_json += ",";

         ulong ticket = PositionGetTicket(i);
         string direction = (PositionGetInteger(POSITION_TYPE) == POSITION_TYPE_BUY) ? "buy" : "sell";
         double entry_price = PositionGetDouble(POSITION_PRICE_OPEN);
         double sl = PositionGetDouble(POSITION_SL);
         double tp = PositionGetDouble(POSITION_TP);
         double lots = PositionGetDouble(POSITION_VOLUME);
         datetime entry_time = (datetime)PositionGetInteger(POSITION_TIME);
         string mode = "scalp"; // Default mode, server can override

         open_positions_json += StringFormat(
                                   "{\"ticket\":%llu,\"symbol\":\"%s\",\"direction\":\"%s\",\"entryPrice\":%.5f,\"sl\":%.5f,\"tp\":%.5f,\"lotSize\":%.2f,\"entryTimestamp\":\"%s\",\"mode\":\"%s\"}",
                                   ticket, _Symbol, direction, entry_price, sl, tp, lots, TimeToString(entry_time, TIME_DATE | TIME_SECONDS), mode
                                );
         open_pos_count++;
        }
     }
   open_positions_json += "]";

// Build the JSON payload
   string m1_opens_str = "", m1_closes_str = "", m1_highs_str = "", m1_lows_str = "", m1_volumes_str = "";
   string m5_closes_str = "", m5_highs_str = "", m5_lows_str = "";
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
   for(int i = 0; i < ArraySize(m30_rates); i++)
     {
      m30_closes_str += DoubleToString(m30_rates[i].close, _Digits);
      if(i < ArraySize(m30_rates) - 1)
         m30_closes_str += ",";
     }
   for(int i = 0; i < ArraySize(h1_rates); i++)
     {
      h1_closes_str += DoubleToString(h1_rates[i].close, _Digits);
      h1_highs_str += DoubleToString(h1_rates[i].high, _Digits);
      h1_lows_str += DoubleToString(h1_rates[i].low, _Digits);
      if(i < ArraySize(h1_rates) - 1)
        {
         h1_closes_str += ",";
         h1_highs_str += ",";
         h1_lows_str += ",";
        }
     }
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

   long last_m1_timestamp = (long)m1_rates[ArraySize(m1_rates)-1].time;

   string json_payload = StringFormat(
                            "{\"symbol\":\"%s\",\"timeframe\":\"M1\","
                            "\"currentPrice\":%.5f,\"spreadPoints\":%.1f,\"priceDecimals\":%d,\"lastM1Timestamp\":%lld,"
                            "\"opens\":[%s],\"closes\":[%s],\"highs\":[%s],\"lows\":[%s],\"volumes\":[%s],"
                            "\"m5Closes\":[%s],\"m5Highs\":[%s],\"m5Lows\":[%s],"
                            "\"m30Closes\":[%s],"
                            "\"h1Closes\":[%s],\"h1Highs\":[%s],\"h1Lows\":[%s],"
                            "\"h4Closes\":[%s],\"h4Highs\":[%s],\"h4Lows\":[%s],"
                            "\"d1Opens\":[%s],\"d1Closes\":[%s],"
                            "\"openPositions\":%s,"
                            "\"rsiPeriod\":%d,\"emaFast\":%d,\"emaSlow\":%d,\"atrPeriod\":%d,\"smaPeriod\":%d,"
                            "\"spreadLimitPoints\":%.1f,"
                            "\"stochKPeriod\":%d,\"stochDPeriod\":%d,\"stochSlowing\":%d,"
                            "\"slAtrMultiplier\":%.2f,\"tpAtrMultiplier\":%.2f,"
                            "\"adxPeriod\":%d,\"adxThreshold\":%.1f,"
                            "\"chandelierPeriod\":%d,\"chandelierAtrMult\":%.1f,"
                            "\"maxHoldBars\":%d,"
                            "\"mode\":\"scalp\"}",
                            _Symbol,
                            ask, spread_pts, _Digits, last_m1_timestamp,
                            m1_opens_str, m1_closes_str, m1_highs_str, m1_lows_str, m1_volumes_str,
                            m5_closes_str, m5_highs_str, m5_lows_str, m30_closes_str,
                            h1_closes_str, h1_highs_str, h1_lows_str,
                            h4_closes_str, h4_highs_str, h4_lows_str,
                            d1_opens_str, d1_closes_str,
                            open_positions_json,
                            RsiPeriod, EmaFastPeriod, EmaSlowPeriod, AtrPeriod, SmaPeriod, MaxSpreadPoints,
                            StochKPeriod, StochDPeriod, StochSlowing,
                            SlAtrMultiplier, TpAtrMultiplier,
                            AdxPeriod, AdxThreshold, ChandelierPeriod, ChandelierAtrMult, MaxHoldBars
                         );

// --- 1. POST data to the Rust server ---
   ResetLastError();
   string result_headers;
   StringToCharArray(json_payload, post_data);
   int res = WebRequest("POST", ServerUrl + "/data", "Content-Type: application/json", 5000, post_data, result, result_headers);

   string action = "hold";
   string reason = "No Signal";
   string entry_type = "none";
   string tp_pips = "-";
   string sl_pips = "-";
   double sl_price = 0.0;
   double limit_price = 0.0;
   string recommended_order_type = "none";
   long expiration_seconds = 0;

   double tp1_price = 0.0;
   ulong ticket_to_manage = 0;
   double new_sl_price = 0.0;


   if(res == -1)
     {
      Print("WebRequest failed. Error code: ", GetLastError());
      reason = "POST /data Failed";
     }
   else
      if(res != 200)
        {
         Print("Server /data returned non-200 status: ", res);
         Print("Server response: ", CharArrayToString(result));
         reason = "Server Error " + IntegerToString(res);
        }
      else
        {
         // --- 2. GET signals from the server ---
         Print("Data POST successful. Now GETting signals.");
         char get_result[];
         string get_headers;
         int get_res = WebRequest("GET", ServerUrl + "/signals/" + _Symbol, NULL, 5000, post_data, get_result, get_headers);

         if(get_res != 200)
           {
            Print("WebRequest to /signals failed. Status: ", get_res);
            reason = "GET /signals Failed";
           }
         else
           {
            // --- Process server response ---
            string response_str = CharArrayToString(get_result);
            // We are interested in the scalp signal from the nested response
            entry_type = GetNestedJsonValue(response_str, "scalpSignal", "entryType", true);
            reason = GetNestedJsonValue(response_str, "scalpSignal", "reason", true);
            sl_price = StringToDouble(GetNestedJsonValue(response_str, "scalpSignal", "slPrice", false));
            tp1_price = StringToDouble(GetNestedJsonValue(response_str, "scalpSignal", "tp1Price", false));
            recommended_order_type = GetNestedJsonValue(response_str, "scalpSignal", "recommendedOrderType", true);
            limit_price = StringToDouble(GetNestedJsonValue(response_str, "scalpSignal", "limitOrderPrice", false));
            expiration_seconds = (long)StringToInteger(GetNestedJsonValue(response_str, "scalpSignal", "expirationSeconds", false));

            // --- NEW: Process swingSignal for position management ---
            action = GetNestedJsonValue(response_str, "swingSignal", "action", true);
            ticket_to_manage = (ulong)StringToInteger(GetNestedJsonValue(response_str, "swingSignal", "ticketToManage", false));
            new_sl_price = StringToDouble(GetNestedJsonValue(response_str, "swingSignal", "slPrice", false));

            // For display purposes, convert prices to pips
            if(entry_type == "long")
              {
               sl_pips = DoubleToString((ask - sl_price) / (point_val * 10.0), 1);
               tp_pips = DoubleToString((tp1_price - ask) / (point_val * 10.0), 1);
              }
            else
               if(entry_type == "short")
                 {
                  sl_pips = DoubleToString((sl_price - bid) / (point_val * 10.0), 1);
                  tp_pips = DoubleToString((bid - tp1_price) / (point_val * 10.0), 1);
                 }

            PrintFormat("Server response: entryType=%s, reason=%s, sl=%.5f, tp1=%.5f", entry_type, reason, sl_price, tp1_price);
           }
        }

// --- Position Management Actions (Highest Priority) ---
   if(action == "close" && ticket_to_manage > 0)
     {
      PrintFormat("Server advised to CLOSE position #%llu", ticket_to_manage);
      trade.PositionClose(ticket_to_manage);
     }
   else
      if(action == "update_sl" && ticket_to_manage > 0 && new_sl_price > 0)
        {
         if(PositionSelectByTicket(ticket_to_manage))
           {
            PrintFormat("Server advised to UPDATE SL for position #%llu to %.5f", ticket_to_manage, new_sl_price);
            // Modify the position with the new SL, keeping the existing TP
            trade.PositionModify(ticket_to_manage, new_sl_price, PositionGetDouble(POSITION_TP));
           }
        }
// --- End of Position Management ---

// --- NEW: Calculate Lot Size based on Conviction Score ---
   double lot_size = 0.01; // Default to 0.01, will be replaced by server logic later

// --- Display graphical dashboard on chart ---
   UpdateDashboard(entry_type, reason, tp_pips, sl_pips, spread_pts, AccountInfoDouble(ACCOUNT_BALANCE), current_pl, lot_size);


// --- Trade Execution Logic ---
   if(StringFind(recommended_order_type, "long") >= 0 && lot_size > 0.0 && action != "close" && action != "update_sl")
     {
      // Close any opposing positions before opening a new one
      ClosePositions(POSITION_TYPE_SELL);

      int open_buys = CountOpenPositions(POSITION_TYPE_BUY);
      bool can_pyramid = EnablePyramiding && open_buys > 0 && open_buys < MaxPyramidEntries;
      bool is_profitable_enough = false;

      if(can_pyramid)
        {
         double total_profit = 0;
         for(int i = PositionsTotal() - 1; i >= 0; i--)
           {
            if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber && PositionGetInteger(POSITION_TYPE) == POSITION_TYPE_BUY)
              {
               total_profit += PositionGetDouble(POSITION_PROFIT);
              }
           }
         is_profitable_enough = (total_profit / (point_val * 10.0)) > PyramidProfitPips;
         // Note: Pyramiding with fixed lot sizes might need a different logic if desired.
         // For now, it will use the same conviction-based lot size for the new entry.
        }

      if(open_buys == 0 || (can_pyramid && is_profitable_enough))
        {
         if(lot_size > 0)
           {
            if(recommended_order_type == "limit_buy")
            {
               datetime expiration = (expiration_seconds > 0) ? TimeCurrent() + expiration_seconds : 0;
               if(trade.BuyLimit(lot_size, limit_price, _Symbol, sl_price, tp1_price, ORDER_TIME_GTC, expiration, "XAU Bridge Limit BUY"))
               {
                  // Log pending order placement
                  LogEvent("Pending", trade.ResultOrder(), _Symbol, "Buy Limit", lot_size, limit_price, sl_price, tp1_price, 0.0, reason);
               }
            }
            else // Market order
            {
               if(trade.Buy(lot_size, _Symbol, ask, sl_price, tp1_price, "XAU Scalper Bridge BUY"))
               {
                  ulong ticket = trade.ResultDeal();
                  if(PositionSelectByTicket(ticket))
                  {
                     LogEvent("Open", ticket, _Symbol, "Buy", lot_size, PositionGetDouble(POSITION_PRICE_OPEN), sl_price, tp1_price, 0.0, reason);
                  }
               }
            }
           }
        }
     }
   else if(StringFind(recommended_order_type, "short") >= 0 && lot_size > 0.0 && action != "close" && action != "update_sl")
        {
         // Close any opposing positions
         ClosePositions(POSITION_TYPE_BUY);

         int open_sells = CountOpenPositions(POSITION_TYPE_SELL);
         bool can_pyramid = EnablePyramiding && open_sells > 0 && open_sells < MaxPyramidEntries;
         bool is_profitable_enough = false;

         if(can_pyramid)
           {
            double total_profit = 0;
            for(int i = PositionsTotal() - 1; i >= 0; i--)
              {
               if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber && PositionGetInteger(POSITION_TYPE) == POSITION_TYPE_SELL)
                 {
                  total_profit += PositionGetDouble(POSITION_PROFIT);
                 }
              }
            is_profitable_enough = (total_profit / (point_val * 10.0)) > PyramidProfitPips;
           }

         if(open_sells == 0 || (can_pyramid && is_profitable_enough))
           {
            if(lot_size > 0)
            {
               if(recommended_order_type == "limit_sell")
               {
                  datetime expiration = (expiration_seconds > 0) ? TimeCurrent() + expiration_seconds : 0;
                  if(trade.SellLimit(lot_size, limit_price, _Symbol, sl_price, tp1_price, ORDER_TIME_GTC, expiration, "XAU Bridge Limit SELL"))
                  {
                     LogEvent("Pending", trade.ResultOrder(), _Symbol, "Sell Limit", lot_size, limit_price, sl_price, tp1_price, 0.0, reason);
                  }
               }
               else // Market order
               {
                  if(trade.Sell(lot_size, _Symbol, bid, sl_price, tp1_price, "XAU Scalper Bridge SELL"))
                  {
                     ulong ticket = trade.ResultDeal();
                     if(PositionSelectByTicket(ticket))
                     {
                        LogEvent("Open", ticket, _Symbol, "Sell", lot_size, PositionGetDouble(POSITION_PRICE_OPEN), sl_price, tp1_price, 0.0, reason);
                     }
                  }
               }
            }
           }
        }
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void OnTradeTransaction(const MqlTradeTransaction &trans, const MqlTradeRequest &request, const MqlTradeResult &res)
  {
// --- NEW: Log trade closures ---
   if(!EnableServerLogging)
      return;

// We are interested in completed deals that close a position
   if(trans.type == TRADE_TRANSACTION_DEAL_ADD && trans.deal_type == DEAL_TYPE_BUY || trans.deal_type == DEAL_TYPE_SELL)
     {
      // A deal is added to history. Check if it closes a position.
      // We can check this by looking for a corresponding position ticket in the deal.
      if(HistoryDealSelect(trans.deal))
        {
         long position_id = HistoryDealGetInteger(trans.deal, DEAL_POSITION_ID);
         if(PositionSelectByTicket(position_id))
           {
            // Position still exists, this was an entry deal.
            return;
           }
         else
           {
            // Position does not exist, this was a closing deal.
            if(HistoryDealGetInteger(trans.deal, DEAL_MAGIC) == MagicNumber)
              {
               string deal_symbol = HistoryDealGetString(trans.deal, DEAL_SYMBOL);
               if(deal_symbol == _Symbol)
                 {
                  ulong ticket = HistoryDealGetInteger(trans.deal, DEAL_TICKET);
                  string direction = (HistoryDealGetInteger(trans.deal, DEAL_TYPE) == DEAL_TYPE_BUY) ? "Buy Close" : "Sell Close";
                  double lots = HistoryDealGetDouble(trans.deal, DEAL_VOLUME);
                  double price = HistoryDealGetDouble(trans.deal, DEAL_PRICE);
                  double profit = HistoryDealGetDouble(trans.deal, DEAL_PROFIT);
                  string comment = HistoryDealGetString(trans.deal, DEAL_COMMENT);

                  // Log the closing event
                  LogEvent("Close", ticket, deal_symbol, direction, lots, price, 0.0, 0.0, profit, comment);

                 }
              }
           }
        }
     }
  }

// --- Trade Management Functions ---

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void ClosePositions(ENUM_POSITION_TYPE direction)
  {
   for(int i = PositionsTotal() - 1; i >= 0; i--)
     {
      if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber && PositionGetInteger(POSITION_TYPE) == direction)
        {
         trade.PositionClose(PositionGetTicket(i));
        }
     }
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
int CountOpenPositions(ENUM_POSITION_TYPE direction)
  {
   int count = 0;
   for(int i = PositionsTotal() - 1; i >= 0; i--)
     {
      if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber && PositionGetInteger(POSITION_TYPE) == direction)
        {
         count++;
        }
     }
   return count;
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void LogEvent(string event_type, ulong ticket, string symbol, string direction, double lot_size, double price, double sl, double tp, double profit, string comment)
  {
   if(!EnableServerLogging)
      return;

// Escape special characters in comment for JSON
   string json_comment = comment;
   StringReplace(json_comment, "\\", "\\\\");
   StringReplace(json_comment, "\"", "\\\"");

   string log_payload = StringFormat(
                           "{\"timestamp\":\"%s\",\"eventType\":\"%s\",\"ticket\":%llu,\"symbol\":\"%s\","
                           "\"direction\":\"%s\",\"lotSize\":%.2f,\"price\":%.5f,\"sl\":%.5f,\"tp\":%.5f,"
                           "\"profit\":%.2f,\"comment\":\"%s\"}",
                           TimeToString(TimeCurrent(), TIME_DATE|TIME_SECONDS), event_type, ticket, symbol,
                           direction, lot_size, price, sl, tp, profit, json_comment
                        );

// Use separate buffers for logging to not interfere with the main eval request
   char log_post_data[];
   char log_result[];
   string log_result_headers;
   StringToCharArray(log_payload, log_post_data); // Corrected function call
   WebRequest("POST", ServerUrl + "/log_trade", "Content-Type: application/json", 1000, log_post_data, log_result, log_result_headers);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color)
  {
   if(ObjectFind(chart_ID, name) < 0)
     {
      ObjectCreate(chart_ID, name, OBJ_RECTANGLE_LABEL, 0, 0, 0);
      ObjectSetInteger(chart_ID, name, OBJPROP_XDISTANCE, x);
      ObjectSetInteger(chart_ID, name, OBJPROP_YDISTANCE, y);
      ObjectSetInteger(chart_ID, name, OBJPROP_XSIZE, width);
      ObjectSetInteger(chart_ID, name, OBJPROP_YSIZE, height);
      ObjectSetInteger(chart_ID, name, OBJPROP_CORNER, CORNER_LEFT_LOWER);
      ObjectSetInteger(chart_ID, name, OBJPROP_BGCOLOR, (long)bg_color);
      ObjectSetInteger(chart_ID, name, OBJPROP_SELECTABLE, false);
      ObjectSetInteger(chart_ID, name, OBJPROP_BACK, true);
     }
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
      ObjectSetInteger(chart_ID, border_name, OBJPROP_XDISTANCE, x);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_YDISTANCE, y);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_XSIZE, width);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_YSIZE, height);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_CORNER, CORNER_LEFT_LOWER);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_BGCOLOR, (long)border_color);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_SELECTABLE, false);
      ObjectSetInteger(chart_ID, border_name, OBJPROP_BACK, true);
     }

// 2. Create the inner rectangle (the background) on top of the border
   CreatePanel(chart_ID, name, x + border_thickness, y + border_thickness, width - (border_thickness * 2), height - (border_thickness * 2), bg_color); // Call the overloaded function
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color)
  {
   if(ObjectFind(chart_ID, name) < 0)
     {
      ObjectCreate(chart_ID, name, OBJ_LABEL, 0, 0, 0);
      ObjectSetInteger(chart_ID, name, OBJPROP_XDISTANCE, x);
      ObjectSetInteger(chart_ID, name, OBJPROP_YDISTANCE, y);
      ObjectSetInteger(chart_ID, name, OBJPROP_CORNER, (long)CORNER_LEFT_LOWER);
      ObjectSetInteger(chart_ID, name, OBJPROP_SELECTABLE, false);
      ObjectSetString(chart_ID, name, OBJPROP_FONT, "Arial");
      ObjectSetInteger(chart_ID, name, OBJPROP_FONTSIZE, 10);
      ObjectSetInteger(chart_ID, name, OBJPROP_BACK, false);
     }
   ObjectSetString(chart_ID, name, OBJPROP_TEXT, text);
   ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, text_color);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateDashboardRow(long chart_ID, string key, string value, int x_key, int x_val, int y, color clr_key, color clr_val)
  {
   string key_name = "XauBridge_" + key;
   string val_name = "XauBridge_" + key + "_Val";

// Create the label for the key (e.g., "RSI")
   CreateLabel(chart_ID, key_name, x_key, y, key, clr_key);
   ObjectSetInteger(chart_ID, key_name, OBJPROP_FONTSIZE, 9);
   ObjectSetString(chart_ID, key_name, OBJPROP_FONT, "Segoe UI");

// Create the label for the value (e.g., "55.2")
   CreateLabel(chart_ID, val_name, x_val, y, value, clr_val);
   ObjectSetInteger(chart_ID, val_name, OBJPROP_FONTSIZE, 9);
   ObjectSetString(chart_ID, val_name, OBJPROP_FONT, "Segoe UI");
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void UpdateDashboard(string action, string reason, string tp, string sl, double current_spread, double current_balance, double current_pl, double next_lot_size)
  {
   long chart_ID = ChartID();
   int x_pos = 15;
// Adjusted y_pos and panel height for new indicators and conviction score
   int y_pos = 200;
   int y_step = 17;

   color clr_panel_bg = C'33,33,33';
   color clr_panel_border = C'255,193,7'; // Gold accent
   color clr_title = clr_panel_border;
   color clr_label = C'200,200,200'; // Lighter gray for labels
   color clr_value = C'250,250,250'; // White for values
   color clr_spread_ok = C'0,200,83';
   color clr_spread_bad = C'244,67,54';
   color clr_profit = C'0,230,118';
   color clr_loss = C'244,67,54';
   CreatePanel(chart_ID, "XauBridge_Panel", 5, 5, 260, 185, clr_panel_bg, clr_panel_border);

   string signal_text;
   color signal_color;
   if(action == "buy")
     {
      signal_text = "ACTION: 📈 BUY";
      signal_color = C'0,230,118';
     }
   else
      if(action == "sell")
        {
         signal_text = "ACTION: 📉 SELL";
         signal_color = C'244,67,54';
        }
      else
        {
         signal_text = "ACTION: ⏸️ HOLD";
         signal_color = C'120,144,156';
        }

   CreateLabel(chart_ID, "XauBridge_Title", x_pos, y_pos, "🌉 XAU SCALPER BRIDGE", clr_title);
   ObjectSetString(chart_ID, "XauBridge_Title", OBJPROP_FONT, "Segoe UI Semibold");
   ObjectSetInteger(chart_ID, "XauBridge_Title", OBJPROP_FONTSIZE, 12);
   y_pos -= y_step + 4;

   CreateLabel(chart_ID, "XauBridge_Signal", x_pos, y_pos, signal_text, signal_color);
   ObjectSetString(chart_ID, "XauBridge_Signal", OBJPROP_FONT, "Segoe UI");
   ObjectSetInteger(chart_ID, "XauBridge_Signal", OBJPROP_FONTSIZE, 10);
   y_pos -= y_step;
   CreateLabel(chart_ID, "XauBridge_Reason", x_pos, y_pos, "Reason: " + reason, clr_label);
   ObjectSetString(chart_ID, "XauBridge_Reason", OBJPROP_FONT, "Segoe UI");
   ObjectSetInteger(chart_ID, "XauBridge_Reason", OBJPROP_FONTSIZE, 9);
   y_pos -= y_step + 7;

// --- Two-column layout ---
   int x_val_pos = x_pos + 130;

// Market Info
   CreateDashboardRow(chart_ID, "Balance", DoubleToString(current_balance, 2), x_pos, x_val_pos, y_pos, clr_label, clr_value);
   y_pos -= y_step;
   CreateDashboardRow(chart_ID, "Spread", DoubleToString(current_spread, 1) + " pts", x_pos, x_val_pos, y_pos, clr_label, (current_spread <= MaxSpreadPoints ? clr_spread_ok : clr_spread_bad));
   y_pos -= y_step + 7;

// --- NEW: Current P/L Row ---
   string pl_string = (current_pl == 0.0 && PositionsTotal() == 0) ? "---" : DoubleToString(current_pl, 2);
   color pl_color = (current_pl > 0) ? clr_profit : (current_pl < 0 ? clr_loss : clr_value);
   CreateDashboardRow(chart_ID, "Current P/L", pl_string, x_pos, x_val_pos, y_pos, clr_label, pl_color);
   y_pos -= y_step + 7;

// Trade Plan
   CreateDashboardRow(chart_ID, "Take Profit Pips", tp, x_pos, x_val_pos, y_pos, clr_label, clr_value);
   y_pos -= y_step;
   CreateDashboardRow(chart_ID, "Stop Loss Pips", sl, x_pos, x_val_pos, y_pos, clr_label, clr_value);
   y_pos -= y_step;
// --- NEW: Next Lot Size Row ---
   CreateDashboardRow(chart_ID, "Next Lot Size", DoubleToString(next_lot_size, 2), x_pos, x_val_pos, y_pos, clr_label, clr_value);
   y_pos -= y_step;

   ChartRedraw(chart_ID);
  }

// --- Calculation Functions ---

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
double CalculateLotSize(double stop_loss_pips, string symbol, double point_value, double risk_percentage_override)
  {
   if(stop_loss_pips <= 0)
     {
      Print("Invalid stop loss (<= 0), cannot calculate lot size.");
      return 0.0;
     }

   double balance = AccountInfoDouble(ACCOUNT_BALANCE);
   double risk_amount = balance * (risk_percentage_override / 100.0); // Use override

   double contract_size = SymbolInfoDouble(symbol, SYMBOL_TRADE_CONTRACT_SIZE);
   double tick_size = SymbolInfoDouble(symbol, SYMBOL_TRADE_TICK_SIZE);
   double tick_value = SymbolInfoDouble(symbol, SYMBOL_TRADE_TICK_VALUE);

   if(contract_size <= 0 || tick_size <= 0 || tick_value <= 0)
     {
      Print("Invalid symbol properties for lot size calculation.");
      return 0.0;
     }

   double stop_loss_in_price = stop_loss_pips * point_value * 10.0;
   double loss_per_lot = stop_loss_in_price / tick_size * tick_value;

   double lot_size = (loss_per_lot > 0) ? risk_amount / loss_per_lot : 0.0;

   double min_vol = SymbolInfoDouble(symbol, SYMBOL_VOLUME_MIN);
   double max_vol = SymbolInfoDouble(symbol, SYMBOL_VOLUME_MAX);
   double vol_step = SymbolInfoDouble(symbol, SYMBOL_VOLUME_STEP);

   if(vol_step > 0)
     {
      lot_size = MathRound(lot_size / vol_step) * vol_step;
     }

   if(lot_size < min_vol)
      lot_size = min_vol;
   if(max_vol > 0 && lot_size > max_vol)
      lot_size = max_vol;

   return lot_size;
  }

//+------------------------------------------------------------------+
//| Calculate Lot Size based on Conviction Score                     |
//+------------------------------------------------------------------+
double CalculateLotSizeConviction(int conviction)
  {
   double min_lot = 0.01;
   double max_lot = 0.05;
   int min_conviction = 50;
   int max_conviction = 100;

   if(conviction < min_conviction)
     {
      return 0.0; // No trade if conviction is too low
     }

// Normalize conviction score to a 0.0-1.0 range
   double conviction_percentage = (double)(conviction - min_conviction) / (double)(max_conviction - min_conviction);
   conviction_percentage = MathMax(0.0, MathMin(1.0, conviction_percentage)); // Clamp between 0 and 1

// Linearly scale lot size
   double lot_size = min_lot + (max_lot - min_lot) * conviction_percentage;

// --- Normalize to broker's volume rules ---
   double min_vol = SymbolInfoDouble(_Symbol, SYMBOL_VOLUME_MIN);
   double max_vol = SymbolInfoDouble(_Symbol, SYMBOL_VOLUME_MAX);
   double vol_step = SymbolInfoDouble(_Symbol, SYMBOL_VOLUME_STEP);

   if(vol_step > 0)
     {
      lot_size = MathRound(lot_size / vol_step) * vol_step;
     }

// Ensure it's within broker limits
   if(lot_size < min_vol)
      lot_size = min_vol;
   if(max_vol > 0 && lot_size > max_vol)
      lot_size = max_vol;

   return lot_size;
  }


//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
string GetJsonValue(string json, string key, bool is_string)
  {
   string search_key;
   string end_char;

   if(is_string)
     {
      search_key = "\"" + key + "\":\"";
      end_char = "\"";
     }
   else
     {
      search_key = "\"" + key + "\":";
      end_char = ",";
     }

   int start_pos = StringFind(json, search_key);
   if(start_pos < 0)
      return "";

   start_pos += StringLen(search_key);
   int end_pos = -1;

   if(is_string)
     {
      end_pos = StringFind(json, end_char, start_pos);
     }
   else // It's a number, so it can be terminated by a comma or the end of the object
     {
      int comma_pos = StringFind(json, ",", start_pos);
      int brace_pos = StringFind(json, "}", start_pos);

      if(comma_pos > 0 && brace_pos > 0)
         end_pos = MathMin(comma_pos, brace_pos);
      else
         if(comma_pos > 0)
            end_pos = comma_pos;
         else
            end_pos = brace_pos;
     }

   if(end_pos < 0)
      return "";

   return StringSubstr(json, start_pos, end_pos - start_pos);
  }

//+------------------------------------------------------------------+
//| Get a value from a nested JSON object.                           |
//| e.g., GetNestedJsonValue(json, "scalpSignal", "entryType", true) |
//+------------------------------------------------------------------+
string GetNestedJsonValue(string json, string object_key, string value_key, bool is_string)
  {
// 1. Find the start of the nested object
   string search_object_key = "\"" + object_key + "\":{";
   int object_start_pos = StringFind(json, search_object_key);
   if(object_start_pos < 0)
     {
      // Check if the object is null
      search_object_key = "\"" + object_key + "\":null";
      if(StringFind(json, search_object_key) >= 0)
        {
         return is_string ? "none" : "0.0"; // Return default values if object is null
        }
      return ""; // Object not found
     }

   object_start_pos += StringLen(search_object_key);

// 2. Find the end of the nested object by matching braces
   int brace_count = 1;
   int object_end_pos = -1;
   for(int i = object_start_pos; i < StringLen(json); i++)
     {
      if(StringGetCharacter(json, i) == '{')
         brace_count++;
      if(StringGetCharacter(json, i) == '}')
         brace_count--;
      if(brace_count == 0)
        {
         object_end_pos = i;
         break;
        }
     }
   if(object_end_pos < 0)
      return ""; // Malformed JSON

// 3. Extract the nested object and get the value from it
   string nested_json = StringSubstr(json, object_start_pos - 1, object_end_pos - (object_start_pos - 1));
   return GetJsonValue("{" + nested_json, value_key, is_string);
  }
//+------------------------------------------------------------------+
