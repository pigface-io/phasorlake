-- ************************************************************ 
-- Title:	PUBLIC_DVD_TRADINGPRICE_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS TRADINGPRICE owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_TRADINGPRICE_201501010000.csv  
APPEND INTO TABLE TRADINGPRICE  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
RUNNO FLOAT EXTERNAL, 
REGIONID CHAR(10), 
PERIODID FLOAT EXTERNAL, 
RRP FLOAT EXTERNAL, 
EEP FLOAT EXTERNAL, 
INVALIDFLAG CHAR(1), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
ROP FLOAT EXTERNAL, 
RAISE6SECRRP FLOAT EXTERNAL, 
RAISE6SECROP FLOAT EXTERNAL, 
RAISE60SECRRP FLOAT EXTERNAL, 
RAISE60SECROP FLOAT EXTERNAL, 
RAISE5MINRRP FLOAT EXTERNAL, 
RAISE5MINROP FLOAT EXTERNAL, 
RAISEREGRRP FLOAT EXTERNAL, 
RAISEREGROP FLOAT EXTERNAL, 
LOWER6SECRRP FLOAT EXTERNAL, 
LOWER6SECROP FLOAT EXTERNAL, 
LOWER60SECRRP FLOAT EXTERNAL, 
LOWER60SECROP FLOAT EXTERNAL, 
LOWER5MINRRP FLOAT EXTERNAL, 
LOWER5MINROP FLOAT EXTERNAL, 
LOWERREGRRP FLOAT EXTERNAL, 
LOWERREGROP FLOAT EXTERNAL, 
PRICE_STATUS CHAR(20)) 
-- End of Script 
