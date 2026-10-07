-- ************************************************************ 
-- Title:	PUBLIC_DVD_SET_FCAS_REGULATION_TRK_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS SET_FCAS_REGULATION_TRK owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_SET_FCAS_REGULATION_TRK_201501010000.csv  
APPEND INTO TABLE SET_FCAS_REGULATION_TRK	
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
VERSIONNO FLOAT EXTERNAL, 
INTERVAL_DATETIME DATE "yyyy/mm/dd hh24:mi:ss", 
CONSTRAINTID CHAR(20), 
CMPF FLOAT EXTERNAL, 
CRMPF FLOAT EXTERNAL, 
RECOVERY_FACTOR_CMPF FLOAT EXTERNAL, 
RECOVERY_FACTOR_CRMPF FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
