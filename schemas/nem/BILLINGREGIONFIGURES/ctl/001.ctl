-- ************************************************************ 
-- Title:	PUBLIC_DVD_BILLINGREGIONFIGURES_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS BILLINGREGIONFIGURES owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_BILLINGREGIONFIGURES_201501010000.csv  
APPEND INTO TABLE BILLINGREGIONFIGURES  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
CONTRACTYEAR FLOAT EXTERNAL, 
WEEKNO FLOAT EXTERNAL, 
BILLRUNNO FLOAT EXTERNAL, 
REGIONID CHAR(10), 
ENERGYOUT FLOAT EXTERNAL, 
VALUEOUT FLOAT EXTERNAL, 
ENERGYPURCHASED FLOAT EXTERNAL, 
VALUEPURCHASED FLOAT EXTERNAL, 
EXCESSGEN FLOAT EXTERNAL, 
RESERVETRADING FLOAT EXTERNAL, 
INTCOMPO FLOAT EXTERNAL, 
ADMINPRICECOMPO FLOAT EXTERNAL, 
SETTSURPLUS FLOAT EXTERNAL, 
ASPAYMENT FLOAT EXTERNAL, 
POOLFEES FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
