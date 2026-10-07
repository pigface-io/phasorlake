-- ************************************************************ 
-- Title:	PUBLIC_DVD_P5MIN_INTERCONNECTORSOLN_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS P5MIN_INTERCONNECTORSOLN owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_P5MIN_INTERCONNECTORSOLN_201501010000.csv  
APPEND INTO TABLE P5MIN_INTERCONNECTORSOLN  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
RUN_DATETIME DATE "yyyy/mm/dd hh24:mi:ss", 
INTERCONNECTORID CHAR(10), 
INTERVAL_DATETIME DATE "yyyy/mm/dd hh24:mi:ss", 
METEREDMWFLOW FLOAT EXTERNAL, 
MWFLOW FLOAT EXTERNAL, 
MWLOSSES FLOAT EXTERNAL, 
MARGINALVALUE FLOAT EXTERNAL, 
VIOLATIONDEGREE FLOAT EXTERNAL, 
MNSP FLOAT EXTERNAL, 
EXPORTLIMIT FLOAT EXTERNAL, 
IMPORTLIMIT FLOAT EXTERNAL, 
MARGINALLOSS FLOAT EXTERNAL, 
EXPORTGENCONID CHAR(20), 
IMPORTGENCONID CHAR(20), 
FCASEXPORTLIMIT FLOAT EXTERNAL, 
FCASIMPORTLIMIT FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
LOCAL_PRICE_ADJUSTMENT_EXPORT FLOAT EXTERNAL, 
LOCALLY_CONSTRAINED_EXPORT FLOAT EXTERNAL, 
LOCAL_PRICE_ADJUSTMENT_IMPORT FLOAT EXTERNAL, 
LOCALLY_CONSTRAINED_IMPORT FLOAT EXTERNAL) 
-- End of Script 
