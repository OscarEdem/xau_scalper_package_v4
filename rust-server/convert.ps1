
# This script converts the tab-delimited data exported from MT5
# into the comma-separated format required by the backtester.

Import-Csv -Path '.\xauusd_m1_data.csv' -Delimiter "`t" |
Select-Object -Property @{Name='time';Expression={"{0} {1}" -f $_.'<DATE>',$_.'<TIME>'}},
                        @{Name='open';Expression={$_.'<OPEN>'}},
                        @{Name='high';Expression={$_.'<HIGH>'}},
                        @{Name='low';Expression={$_.'<LOW>'}},
                        @{Name='close';Expression={$_.'<CLOSE>'}},
                        @{Name='volume';Expression={$_.'<TICKVOL>'}} |
Export-Csv -Path '.\data.csv' -NoTypeInformation
