-- ************************************************************ 
-- Title:	PUBLIC_DVD_BILLING_CO2E_PUBLICATION_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS BILLING_CO2E_PUBLICATION owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_BILLING_CO2E_PUBLICATION_201501010000.csv  
APPEND INTO TABLE BILLING_CO2E_PUBLICATION  
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
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
REGIONID CHAR(20), 
SENTOUTENERGY FLOAT EXTERNAL, 
GENERATOREMISSIONS FLOAT EXTERNAL, 
INTENSITYINDEX FLOAT EXTERNAL) 
-- End of Script 
