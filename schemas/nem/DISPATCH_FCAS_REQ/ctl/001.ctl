-- ************************************************************ 
-- Title:	PUBLIC_DVD_DISPATCH_FCAS_REQ_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS DISPATCH_FCAS_REQ owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_DISPATCH_FCAS_REQ_201501010000.csv  
APPEND INTO TABLE DISPATCH_FCAS_REQ  
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
INTERVENTION FLOAT EXTERNAL, 
GENCONID CHAR(20), 
REGIONID CHAR(10), 
BIDTYPE CHAR(10), 
GENCONEFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss", 
GENCONVERSIONNO FLOAT EXTERNAL, 
MARGINALVALUE FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
BASE_COST FLOAT EXTERNAL, 
ADJUSTED_COST FLOAT EXTERNAL, 
ESTIMATED_CMPF FLOAT EXTERNAL, 
ESTIMATED_CRMPF FLOAT EXTERNAL, 
RECOVERY_FACTOR_CMPF FLOAT EXTERNAL, 
RECOVERY_FACTOR_CRMPF FLOAT EXTERNAL) 
-- End of Script 
