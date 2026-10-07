-- ************************************************************ 
-- Title:	PUBLIC_DVD_TRADINGINTERCONNECT_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS TRADINGINTERCONNECT owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_TRADINGINTERCONNECT_201501010000.csv  
APPEND INTO TABLE TRADINGINTERCONNECT  
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
INTERCONNECTORID CHAR(10), 
PERIODID FLOAT EXTERNAL, 
METEREDMWFLOW FLOAT EXTERNAL, 
MWFLOW FLOAT EXTERNAL, 
MWLOSSES FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
